use crate::hbbs_http::create_http_client_with_url;
use bytes::Bytes;
use hbb_common::{bail, config::Config, lazy_static, log, ResultType};
use reqwest::blocking::{Body, Client};
use scrap::record::RecordState;
use serde::Serialize;
use serde_json::Map;
use std::{
    fs::File,
    io::{prelude::*, SeekFrom},
    sync::{mpsc::Receiver, Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_HEADER_LEN: usize = 1024;
const SHOULD_SEND_TIME: Duration = Duration::from_secs(1);
const SHOULD_SEND_SIZE: u64 = 1024 * 1024;
const MAX_CHUNK_SIZE: usize = 1024 * 1024;

lazy_static::lazy_static! {
    static ref ENABLE: Arc<Mutex<bool>> = Default::default();
}

pub fn is_enable() -> bool {
    ENABLE.lock().unwrap().clone()
}

pub fn run(rx: Receiver<RecordState>) {
    std::thread::spawn(move || {
        let api_server = crate::get_api_server(
            Config::get_option("api-server"),
            Config::get_option("custom-rendezvous-server"),
        );
        // This URL is used for TLS connectivity testing and fallback detection.
        let login_option_url = format!("{}/api/login-options", &api_server);
        let client = create_http_client_with_url(&login_option_url);
        let mut uploader = RecordUploader {
            client,
            api_server,
            filepath: Default::default(),
            filename: Default::default(),
            upload_size: Default::default(),
            running: Default::default(),
            last_send: Instant::now(),
        };
        loop {
            if let Err(e) = match rx.recv() {
                Ok(state) => match state {
                    RecordState::NewFile(filepath) => uploader.handle_new_file(filepath),
                    RecordState::NewFrame => {
                        if uploader.running {
                            uploader.handle_frame(false)
                        } else {
                            Ok(())
                        }
                    }
                    RecordState::WriteTail => {
                        if uploader.running {
                            uploader.handle_tail()
                        } else {
                            Ok(())
                        }
                    }
                    RecordState::RemoveFile => {
                        if uploader.running {
                            uploader.handle_remove()
                        } else {
                            Ok(())
                        }
                    }
                },
                Err(e) => {
                    log::trace!("upload thread stop: {}", e);
                    break;
                }
            } {
                uploader.running = false;
                log::error!("upload stop: {}", e);
            }
        }
    });
}

struct RecordUploader {
    client: Client,
    api_server: String,
    filepath: String,
    filename: String,
    upload_size: u64,
    running: bool,
    last_send: Instant,
}
impl RecordUploader {
    fn send<Q, B>(&self, query: &Q, body: B) -> ResultType<()>
    where
        Q: Serialize + ?Sized,
        B: Into<Body>,
    {
        match self
            .client
            .post(format!("{}/api/record", self.api_server))
            .query(query)
            .body(body)
            .send()
        {
            Ok(resp) => {
                if let Ok(m) = resp
                    .error_for_status()?
                    .json::<Map<String, serde_json::Value>>()
                {
                    if let Some(e) = m.get("error") {
                        bail!(e.to_string());
                    }
                }
                Ok(())
            }
            Err(e) => bail!(e.to_string()),
        }
    }

    fn handle_new_file(&mut self, filepath: String) -> ResultType<()> {
        match std::path::PathBuf::from(&filepath).file_name() {
            Some(filename) => match filename.to_owned().into_string() {
                Ok(filename) => {
                    self.filename = filename.clone();
                    self.filepath = filepath.clone();
                    self.upload_size = 0;
                    self.running = true;
                    self.last_send = Instant::now();
                    self.send(&[("type", "new"), ("file", &filename)], Bytes::new())?;
                    Ok(())
                }
                Err(_) => bail!("can't parse filename:{:?}", filename),
            },
            None => bail!("can't parse filepath:{}", filepath),
        }
    }

    fn handle_frame(&mut self, flush: bool) -> ResultType<()> {
        if !flush && self.last_send.elapsed() < SHOULD_SEND_TIME {
            return Ok(());
        }
        let mut file = File::open(&self.filepath)?;
        // Upload a fixed snapshot: the recorder can append while requests are in flight.
        let len = file.metadata()?.len();
        if len <= self.upload_size {
            return Ok(());
        }
        if !flush && len - self.upload_size < SHOULD_SEND_SIZE {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.upload_size))?;
        while self.upload_size < len {
            let length = (len - self.upload_size).min(MAX_CHUNK_SIZE as u64) as usize;
            let mut buf = vec![0; length];
            file.read_exact(&mut buf)?;
            self.send(
                &[
                    ("type", "part"),
                    ("file", &self.filename),
                    ("offset", &self.upload_size.to_string()),
                    ("length", &length.to_string()),
                ],
                buf,
            )?;
            // Advance only by bytes acknowledged by the server, including on partial failure.
            self.upload_size += length as u64;
            self.last_send = Instant::now();
        }
        Ok(())
    }

    fn handle_tail(&mut self) -> ResultType<()> {
        self.handle_frame(true)?;
        match File::open(&self.filepath) {
            Ok(mut file) => {
                let mut buf = vec![0u8; MAX_HEADER_LEN];
                match file.read(&mut buf) {
                    Ok(length) => {
                        buf.truncate(length);
                        self.send(
                            &[
                                ("type", "tail"),
                                ("file", &self.filename),
                                ("offset", "0"),
                                ("length", &length.to_string()),
                            ],
                            buf,
                        )?;
                        log::info!("upload success, file: {}", self.filename);
                        Ok(())
                    }
                    Err(e) => bail!(e.to_string()),
                }
            }
            Err(e) => bail!(e.to_string()),
        }
    }

    fn handle_remove(&mut self) -> ResultType<()> {
        self.send(
            &[("type", "remove"), ("file", &self.filename)],
            Bytes::new(),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        fs::{self, OpenOptions},
        io::BufReader,
        net::TcpListener,
        path::PathBuf,
        thread::{self, JoinHandle},
    };

    struct RecordingFile(PathBuf);

    impl RecordingFile {
        fn new(data: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!("record-upload-{}", uuid::Uuid::new_v4()));
            fs::write(&path, data).unwrap();
            Self(path)
        }
    }

    impl Drop for RecordingFile {
        fn drop(&mut self) {
            fs::remove_file(&self.0).unwrap();
        }
    }

    struct Request {
        query: HashMap<String, String>,
        body: Vec<u8>,
    }

    fn server(
        responses: Vec<(&'static str, &'static str)>,
        mut before_response: impl FnMut(usize) + Send + 'static,
    ) -> (String, JoinHandle<Vec<Request>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (index, (status, body)) in responses.into_iter().enumerate() {
                let deadline = Instant::now() + Duration::from_secs(10);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "timed out waiting for request");
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => panic!("accept failed: {}", e),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let target = line.split_whitespace().nth(1).unwrap();
                let query = reqwest::Url::parse(&format!("http://localhost{}", target))
                    .unwrap()
                    .query_pairs()
                    .into_owned()
                    .collect();
                let mut length = 0;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut data = vec![0; length];
                reader.read_exact(&mut data).unwrap();
                requests.push(Request { query, body: data });
                before_response(index);
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                )
                .unwrap();
            }
            requests
        });
        (url, handle)
    }

    fn uploader(file: &RecordingFile, api_server: String) -> RecordUploader {
        RecordUploader {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            api_server,
            filepath: file.0.to_str().unwrap().to_owned(),
            filename: "recording.webm".to_owned(),
            upload_size: 0,
            running: true,
            last_send: Instant::now() - SHOULD_SEND_TIME,
        }
    }

    fn assert_part(request: &Request, offset: usize, data: &[u8]) {
        assert_eq!(request.query["type"], "part");
        assert_eq!(request.query["offset"], offset.to_string());
        assert_eq!(request.query["length"], data.len().to_string());
        assert_eq!(request.body, data);
    }

    #[test]
    fn growing_recording_uses_bounded_chunks_and_snapshot_offsets() {
        let data: Vec<u8> = (0..MAX_CHUNK_SIZE * 2 + 7)
            .map(|i| (i % 251) as u8)
            .collect();
        let file = RecordingFile::new(&data);
        let path = file.0.clone();
        let appended = vec![255; 41];
        let appended_by_server = appended.clone();
        let (url, server) = server(vec![("200 OK", ""); 4], move |index| {
            if index == 0 {
                OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(&appended_by_server)
                    .unwrap();
            }
        });
        let mut uploader = uploader(&file, url);
        uploader.handle_frame(false).unwrap();
        assert_eq!(uploader.upload_size, data.len() as u64);
        uploader.handle_frame(true).unwrap();
        assert_eq!(uploader.upload_size, (data.len() + appended.len()) as u64);

        let requests = server.join().unwrap();
        for (index, chunk) in data.chunks(MAX_CHUNK_SIZE).enumerate() {
            assert_part(&requests[index], index * MAX_CHUNK_SIZE, chunk);
        }
        assert_part(&requests[3], data.len(), &appended);
    }

    #[test]
    fn failed_chunk_preserves_acknowledged_offset_for_retry() {
        let data = vec![42; MAX_CHUNK_SIZE + 17];
        let file = RecordingFile::new(&data);
        let (url, server) = server(
            vec![
                ("200 OK", ""),
                ("500 Internal Server Error", "failed"),
                ("200 OK", ""),
            ],
            |_| {},
        );
        let mut uploader = uploader(&file, url);
        assert!(uploader.handle_frame(true).is_err());
        assert_eq!(uploader.upload_size, MAX_CHUNK_SIZE as u64);
        let last_send = uploader.last_send;
        uploader.handle_frame(true).unwrap();
        assert_eq!(uploader.upload_size, data.len() as u64);
        assert!(uploader.last_send >= last_send);

        let requests = server.join().unwrap();
        assert_part(&requests[0], 0, &data[..MAX_CHUNK_SIZE]);
        assert_part(&requests[1], MAX_CHUNK_SIZE, &data[MAX_CHUNK_SIZE..]);
        assert_part(&requests[2], MAX_CHUNK_SIZE, &data[MAX_CHUNK_SIZE..]);
    }

    #[test]
    fn http_and_application_errors_do_not_advance_progress() {
        for (status, body) in [
            ("403 Forbidden", ""),
            ("500 Internal Server Error", "{}"),
            ("200 OK", "{\"error\":\"disk full\"}"),
        ] {
            let file = RecordingFile::new(b"frame");
            let (url, server) = server(vec![(status, body)], |_| {});
            let mut uploader = uploader(&file, url);
            let last_send = uploader.last_send;
            assert!(uploader.handle_frame(true).is_err());
            assert_eq!(uploader.upload_size, 0);
            assert_eq!(uploader.last_send, last_send);
            server.join().unwrap();
        }
    }

    #[test]
    fn tail_flushes_small_recording_before_rewriting_header() {
        let data = b"small recording";
        let file = RecordingFile::new(data);
        let (url, server) = server(vec![("200 OK", ""); 2], |_| {});
        let mut uploader = uploader(&file, url);
        uploader.handle_frame(false).unwrap();
        assert_eq!(uploader.upload_size, 0);
        uploader.last_send = Instant::now();
        uploader.handle_tail().unwrap();
        let requests = server.join().unwrap();
        assert_part(&requests[0], 0, data);
        assert_eq!(requests[1].query["type"], "tail");
        assert_eq!(requests[1].query["offset"], "0");
        assert_eq!(requests[1].body, data);
    }

    #[test]
    fn recent_upload_is_throttled_without_opening_file() {
        let file = RecordingFile::new(&[]);
        let mut uploader = uploader(&file, "http://127.0.0.1:1".to_owned());
        uploader.filepath.clear();
        uploader.last_send = Instant::now();
        uploader.handle_frame(false).unwrap();
        assert!(uploader.handle_frame(true).is_err());
    }

    #[test]
    fn empty_or_fully_uploaded_file_needs_no_request() {
        let file = RecordingFile::new(&[]);
        let mut uploader = uploader(&file, "http://127.0.0.1:1".to_owned());
        uploader.handle_frame(true).unwrap();
        fs::write(&file.0, b"uploaded").unwrap();
        uploader.upload_size = 8;
        uploader.handle_frame(true).unwrap();
    }
}
