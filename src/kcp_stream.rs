use hbb_common::{
    anyhow,
    bytes::{Bytes, BytesMut},
    bytes_codec::BytesCodec,
    config, log,
    tcp::{DynTcpStream, FramedStream},
    tokio::{self, net::UdpSocket, sync::mpsc, sync::oneshot},
    tokio_util, ResultType, Stream,
};
use kcp_sys::{
    endpoint::KcpEndpoint,
    packet_def::{KcpPacket, KcpPacketHeader},
    stream,
};
use std::{net::SocketAddr, sync::Arc};

pub struct KcpStream {
    _endpoint: KcpEndpoint,
    stop_sender: Option<oneshot::Sender<()>>,
}

impl KcpStream {
    fn create_framed(stream: stream::KcpStream, local_addr: Option<SocketAddr>) -> Stream {
        Stream::Tcp(FramedStream(
            tokio_util::codec::Framed::new(DynTcpStream(Box::new(stream)), BytesCodec::new()),
            local_addr.unwrap_or(config::Config::get_any_listen_addr(true)),
            None,
            0,
        ))
    }

    pub async fn accept(
        udp_socket: Arc<UdpSocket>,
        timeout: std::time::Duration,
        init_packet: Option<BytesMut>,
    ) -> ResultType<(Self, Stream)> {
        let mut endpoint = KcpEndpoint::new();
        endpoint.run().await;

        let (input, output) = (
            endpoint.input_sender(),
            endpoint
                .output_receiver()
                .ok_or_else(|| anyhow::anyhow!("Failed to get output receiver"))?,
        );
        let (stop_sender, stop_receiver) = oneshot::channel();
        if let Some(packet) = init_packet {
            if packet.len() >= std::mem::size_of::<KcpPacketHeader>() {
                input.send(packet.into()).await?;
            }
        }
        tokio::spawn(Self::kcp_io(
            udp_socket.clone(),
            input,
            output,
            stop_receiver,
        ));

        let conn_id = tokio::time::timeout(timeout, endpoint.accept()).await??;
        if let Some(stream) = stream::KcpStream::new(&endpoint, conn_id) {
            Ok((
                Self {
                    _endpoint: endpoint,
                    stop_sender: Some(stop_sender),
                },
                Self::create_framed(stream, udp_socket.local_addr().ok()),
            ))
        } else {
            Err(anyhow::anyhow!("Failed to create KcpStream"))
        }
    }

    pub async fn connect(
        udp_socket: Arc<UdpSocket>,
        timeout: std::time::Duration,
    ) -> ResultType<(Self, Stream)> {
        let mut endpoint = KcpEndpoint::new();
        endpoint.run().await;

        let (input, output) = (
            endpoint.input_sender(),
            endpoint
                .output_receiver()
                .ok_or_else(|| anyhow::anyhow!("Failed to get output receiver"))?,
        );
        let (stop_sender, stop_receiver) = oneshot::channel();
        tokio::spawn(Self::kcp_io(
            udp_socket.clone(),
            input,
            output,
            stop_receiver,
        ));

        let conn_id = endpoint.connect(timeout, 0, 0, Bytes::new()).await?;
        if let Some(stream) = stream::KcpStream::new(&endpoint, conn_id) {
            Ok((
                Self {
                    _endpoint: endpoint,
                    stop_sender: Some(stop_sender),
                },
                Self::create_framed(stream, udp_socket.local_addr().ok()),
            ))
        } else {
            Err(anyhow::anyhow!("Failed to create KcpStream"))
        }
    }

    async fn kcp_io(
        udp_socket: Arc<UdpSocket>,
        input: mpsc::Sender<KcpPacket>,
        mut output: mpsc::Receiver<KcpPacket>,
        stop_receiver: oneshot::Receiver<()>,
    ) {
        // Keep both directions and shutdown independently pollable, even when a send blocks.
        tokio::select! {
            _ = stop_receiver => {
                log::debug!("KCP io loop received stop signal");
            }
            _ = input.closed() => {
                log::debug!("KCP endpoint input closed");
            }
            _ = async {
                let mut buf = [0; 1500];
                loop {
                    match udp_socket.recv_from(&mut buf).await {
                        Ok((size, _)) => {
                            if size < std::mem::size_of::<KcpPacketHeader>() {
                                continue;
                            }
                            if input.send(BytesMut::from(&buf[..size]).into()).await.is_err() {
                                log::debug!("KCP endpoint input closed");
                                break;
                            }
                        }
                        Err(e) => {
                            log::debug!("KCP recv_from error: {:?}", e);
                            break;
                        }
                    }
                }
            } => {}
            _ = async {
                while let Some(data) = output.recv().await {
                    if let Err(e) = udp_socket.send(&data.inner()).await {
                        log::debug!("KCP send error: {:?}", e);
                        break;
                    }
                }
            } => {
                log::debug!("KCP endpoint output stopped");
            }
        }
    }
}

impl Drop for KcpStream {
    fn drop(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{poll_fn, Future},
        io::ErrorKind,
        pin::Pin,
        task::Poll,
        time::Duration,
    };
    use tokio::time::timeout;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    struct TestIo {
        socket: Arc<UdpSocket>,
        peer: UdpSocket,
        input_sender: mpsc::Sender<KcpPacket>,
        input: mpsc::Receiver<KcpPacket>,
        output: Option<mpsc::Sender<KcpPacket>>,
        stop: Option<oneshot::Sender<()>>,
        io: Pin<Box<dyn Future<Output = ()> + Send>>,
    }

    impl TestIo {
        async fn new() -> Self {
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            socket.connect(peer.local_addr().unwrap()).await.unwrap();
            peer.connect(socket.local_addr().unwrap()).await.unwrap();
            let (input_sender, input) = mpsc::channel(1);
            let (output, output_receiver) = mpsc::channel(1);
            let (stop, stop_receiver) = oneshot::channel();
            let io = Box::pin(KcpStream::kcp_io(
                socket.clone(),
                input_sender.clone(),
                output_receiver,
                stop_receiver,
            ));
            Self {
                socket,
                peer,
                input_sender,
                input,
                output: Some(output),
                stop: Some(stop),
                io,
            }
        }

        async fn block_input(&mut self) {
            self.input_sender
                .send(KcpPacket::new_with_payload(b"queued"))
                .await
                .unwrap();
            self.peer
                .send(&KcpPacket::new_with_payload(b"pending").inner())
                .await
                .unwrap();
            timeout(TEST_TIMEOUT, self.socket.peek_from(&mut [0; 1500]))
                .await
                .unwrap()
                .unwrap();
            // Poll with a datagram queued so the full input channel blocks the forwarding send.
            poll_fn(|cx| {
                assert!(self.io.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            let error = self.socket.try_recv(&mut [0; 1500]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::WouldBlock);
        }

        async fn finish(self) {
            timeout(TEST_TIMEOUT, self.io)
                .await
                .expect("KCP I/O did not stop");
            if let Some(output) = self.output {
                assert!(output.is_closed());
            }
            assert_eq!(Arc::strong_count(&self.socket), 1, "socket still retained");
        }
    }

    #[tokio::test]
    async fn stop_interrupts_a_blocked_input_send() {
        let mut test = TestIo::new().await;
        test.block_input().await;
        test.stop.take().unwrap().send(()).unwrap();
        test.finish().await;
    }

    #[tokio::test]
    async fn dropping_stop_sender_interrupts_a_blocked_input_send() {
        let mut test = TestIo::new().await;
        test.block_input().await;
        drop(test.stop.take());
        test.finish().await;
    }

    #[tokio::test]
    async fn closed_output_stops_idle_io() {
        let mut test = TestIo::new().await;
        drop(test.output.take());
        test.finish().await;
    }

    #[tokio::test]
    async fn closed_output_interrupts_a_blocked_input_send() {
        let mut test = TestIo::new().await;
        test.block_input().await;
        drop(test.output.take());
        test.finish().await;
    }

    #[tokio::test]
    async fn closed_input_stops_idle_io() {
        let mut test = TestIo::new().await;
        test.input.close();
        test.finish().await;
    }

    #[tokio::test]
    async fn closed_input_interrupts_a_blocked_input_send() {
        let mut test = TestIo::new().await;
        test.block_input().await;
        test.input.close();
        test.finish().await;
    }

    #[tokio::test]
    async fn output_progresses_while_input_is_full_and_input_resumes_in_order() {
        let mut test = TestIo::new().await;
        test.block_input().await;
        let packet = KcpPacket::new_with_payload(b"outgoing");
        let expected = packet.clone().inner();
        test.output.as_ref().unwrap().send(packet).await.unwrap();
        let mut buf = [0; 1500];
        timeout(TEST_TIMEOUT, async {
            tokio::select! {
                _ = &mut test.io => panic!("I/O stopped while input was full"),
                result = test.peer.recv(&mut buf) => {
                    assert_eq!(&buf[..result.unwrap()], &expected[..]);
                }
            }
            assert_eq!(test.input.recv().await.unwrap().payload(), b"queued");
            tokio::select! {
                _ = &mut test.io => panic!("I/O stopped before input resumed"),
                packet = test.input.recv() => {
                    assert_eq!(packet.unwrap().payload(), b"pending");
                }
            }
        })
        .await
        .expect("forwarding stalled under input backpressure");
        test.stop.take().unwrap().send(()).unwrap();
        test.finish().await;
    }

    #[tokio::test]
    async fn short_datagrams_are_ignored_and_valid_packets_are_forwarded() {
        let mut test = TestIo::new().await;
        test.peer.send(b"short").await.unwrap();
        let incoming = KcpPacket::new_with_payload(b"incoming").inner();
        test.peer.send(&incoming).await.unwrap();
        timeout(TEST_TIMEOUT, async {
            tokio::select! {
                _ = &mut test.io => panic!("I/O stopped on a short datagram"),
                packet = test.input.recv() => {
                    assert_eq!(packet.unwrap().inner(), incoming);
                }
            }
        })
        .await
        .unwrap();
        test.stop.take().unwrap().send(()).unwrap();
        test.finish().await;
    }

    #[tokio::test]
    async fn udp_send_error_releases_io_resources() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let (input_sender, _input) = mpsc::channel(1);
        let (output_sender, output) = mpsc::channel(1);
        let (_stop_sender, stop) = oneshot::channel();
        output_sender.send(KcpPacket::new(0)).await.unwrap();
        // Sending on an unconnected socket fails without requiring an external network.
        timeout(
            TEST_TIMEOUT,
            KcpStream::kcp_io(socket.clone(), input_sender, output, stop),
        )
        .await
        .unwrap();
        assert!(output_sender.is_closed());
        assert_eq!(Arc::strong_count(&socket), 1);
    }

    #[tokio::test]
    async fn connected_streams_exchange_frames_and_release_sockets_on_drop() {
        let client_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let server_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        client_socket
            .connect(server_socket.local_addr().unwrap())
            .await
            .unwrap();
        server_socket
            .connect(client_socket.local_addr().unwrap())
            .await
            .unwrap();
        let (client, server) = tokio::join!(
            KcpStream::connect(client_socket.clone(), TEST_TIMEOUT),
            KcpStream::accept(server_socket.clone(), TEST_TIMEOUT, None),
        );
        let (client_guard, mut client) = client.unwrap();
        let (server_guard, mut server) = server.unwrap();
        timeout(TEST_TIMEOUT, async {
            client
                .send_bytes(Bytes::from_static(b"client frame"))
                .await
                .unwrap();
            assert_eq!(&server.next().await.unwrap().unwrap()[..], b"client frame");
            server
                .send_bytes(Bytes::from_static(b"server frame"))
                .await
                .unwrap();
            assert_eq!(&client.next().await.unwrap().unwrap()[..], b"server frame");
        })
        .await
        .unwrap();
        drop((client, server, client_guard, server_guard));
        timeout(TEST_TIMEOUT, async {
            while Arc::strong_count(&client_socket) != 1 || Arc::strong_count(&server_socket) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping the streams retained their UDP sockets");
    }

    #[tokio::test]
    async fn accept_timeout_releases_io_resources() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let result = timeout(
            TEST_TIMEOUT,
            KcpStream::accept(socket.clone(), Duration::from_millis(10), None),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        timeout(TEST_TIMEOUT, async {
            while Arc::strong_count(&socket) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("accept timeout retained the UDP socket");
    }
}
