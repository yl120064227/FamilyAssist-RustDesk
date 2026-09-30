# FamilyAssist-RustDesk 代码检查与修复记录

日期：2026-09-30（Asia/Taipei）

基线：`main` / `6c578292e8ebbbec708b76986ba8c4bc7c509747`，RustDesk 1.4.9。

## 检查范围

本轮重点阅读连接授权范围、登录失败策略、rendezvous 注册、Android 屏幕共享服务、Flutter 服务模型、二维码导入、配置分享与 Windows 便携包生成代码。不是全仓库安全审计。

当前树中未发现家庭协助专用模块、M0 构建验收记录或 8 小时拷机结果。不能据此确认此前所述 Android arm64 / Windows x64 构建验收状态。

## 本轮修复

| 问题 | 原有行为 | 修复结果 |
| --- | --- | --- |
| 二维码先退出、后校验 | 非配置二维码或损坏配置也先触发返回主页面及暂停相机 | 在成功解码后才暂停和退出，非法配置保留扫描页面 |
| 连续扫描重复处理 | 多个扫描事件可进入处理流程，重复导航和弹窗 | 处理有效配置前设置单次处理标记，异步暂停后检查页面是否仍挂载 |
| 配置分享字符损坏 | 把 UTF-16 code units 当成字节，再用 UTF-8 解码 | 使用 UTF-8 编码；ASCII 线格式不变，中文和 emoji 可以往返 |
| 压缩函数改变工作目录 | `os.curdir` 为 `.`，成功返回和异常退出都会留下新的工作目录 | 直接遍历源目录，保留包内相对路径，文件用上下文管理器关闭 |
| 启动文件路径校验不完整 | 字符串前缀比较可接受同名前缀的相邻目录，并允许不存在的文件 | 按真实路径检查目录包含关系，拒绝目录、缺失文件、父目录越界和符号链接越界 |
| 旧版 Windows 打包输入与产物路径错误 | 把安装包输出名当成不存在的启动文件，并移动没有生成的资源文件 | 使用已复制的 `RustDesk.exe`，返回仓库根目录后重命名实际编译产物 |

便携包文件遍历采用稳定顺序，没有修改包的标记、长度字段、压缩算法、MD5 或启动文件相对路径格式。

## 验证与状态

- **PASS**：Linux / Python 3.12 / Brotli 1.2.0 下 12 项便携包回归测试，包括真实压缩解压、异常时工作目录保持、路径边界、CLI 失败时不写包、二进制包格式往返，以及旧版 Windows 打包调用链。调用链测试使用模拟的 Cargo 编译产物，实际执行 Python 打包和重命名，不等同于 Windows 原生编译或程序启动验收。
- **已复现**：同一组工作目录测试在原代码上有 2 项失败；旧字符串前缀判断也会接受源目录外的同名前缀路径。
- **PASS**：修改的 Python 文件语法与新增工作流 YAML 结构检查。
- **新增，待运行**：Flutter 配置分享测试，覆盖 Unicode 往返、ASCII 兼容性和旧版纯 JSON 输入。执行：`cd flutter`，然后 `flutter test test/server_config_test.dart`。
- **新增，待运行**：Windows 2022 / Ubuntu 24.04 的 Python 回归工作流，触发 PR、main 的相关文件推送和手动运行。
- **未执行**：Rust / Flutter 整体编译、Windows 真机便携包启动、Android 相机和服务回归、8 小时拷机与 BBIT。当前执行环境没有 Rust、Dart、Flutter SDK 及目标设备。

便携包回归命令：

```sh
python -m pip install -r libs/portable/requirements.txt
python -m unittest discover -s tests -p test_portable_generate.py -v
```

## 待核查问题与验收条件

1. Android 扫描：普通二维码、`config=损坏内容` 后应继续扫描；有效配置仅弹出一次设置；暂停相机期间手动返回不应再触发导航。相册导入也需回归。
2. Windows：运行上述 Python 测试，并用 Flutter x64 产物生成、启动便携包，验证资源加载及远程连接。
3. Flutter `ServerModel` 每个 FFI 实例都创建周期计时器；计时器没有保存或取消，异步轮询也可能重叠。需要结合会话生命周期验证是否累积后台轮询，再单独修复。
4. 旧 Sciter Windows 打包调用链已修复且通过模拟编译边界的集成测试，仍需在 Windows 上完成真实 Cargo 构建和安装包启动验证。
5. Android `MainService.onDestroy()` 的资源释放和媒体投影撤销回调需结合系统回收、授权撤销、屏幕旋转及真机服务启停验证，不能把静态阅读当作已通过。

合并前至少完成 Flutter 配置测试、Windows Python 回归与 Android 扫描真机验证。长时间连接与 8 小时拷机单独记录结果。
