# iOS 双端构建与使用

`apps/ios` 是原生 SwiftUI 发送 + 接收应用，最低 iOS 17。iOS 26 上使用系统原生 Liquid Glass，旧系统回退到 SwiftUI Material。发送端选择文件后由共享 Rust 核心生成与网页/Android/Windows 完全相同的 RaptorQ 二进制帧，再用 Core Image 显示连续二维码；接收端使用 AVFoundation + Vision 读取二维码原始二进制载荷，交给同一个 Rust C ABI 恢复、校验和组装。

整个传输链路只使用屏幕和相机，不依赖互联网或局域网。接收完成的文件保存在“文件 → 我的 iPhone → AirFerry → 已接收”，应用会自动打开系统预览，也可以直接分享/导出。

## 前置环境

- macOS + Xcode 15 或更新版本
- Rust stable（`rustup` + `cargo`）
- XcodeGen（`brew install xcodegen`）
- 首次安装 Rust crates 与 Apple 编译 target 时需要网络；依赖缓存完成后可离线构建

## 生成并运行

```bash
cd apps/ios
chmod +x scripts/*.sh
./scripts/bootstrap.sh
open AirFerryIOS.xcodeproj
```

在 Xcode 中选择 `AirFerryIOS` target，设置自己的 Signing Team，然后连接真机运行。相机接收必须使用真机；模拟器只适合检查页面与发送端文件选择。

`bootstrap.sh` 会依次：

1. 构建 `transfer-engine --features cffi` 的真机 arm64、Apple Silicon 模拟器和 Intel 模拟器静态库；
2. 用 `xcodebuild -create-xcframework` 生成 `Native/AirFerryCore.xcframework`；
3. 用 `project.yml` 生成 `AirFerryIOS.xcodeproj`。

Rust 或 Swift 接口改动后再次运行 `./scripts/bootstrap.sh` 即可。只想刷新 Rust 库时运行 `./scripts/build-rust-xcframework.sh`。

## GitHub Actions 云构建

仓库根目录已包含 `.github/workflows/ios.yml`。把**整个源码目录的内容**上传到 GitHub 仓库根目录（根目录必须直接看到 `Cargo.toml`、`apps/`、`core/` 和 `.github/`），推送后会自动运行；也可以进入 GitHub 的 **Actions → Build iOS (unsigned) → Run workflow** 手动启动。

流程固定使用 `macos-26` 与 Xcode 26.6，并自动完成 Rust Apple targets、XCFramework、XcodeGen 工程和 iPhone Release 构建。成功后在该次运行底部下载 `AirFerry-iOS-unsigned-运行编号`，压缩包中只包含：

- `AirFerry-unsigned.ipa`

这个 IPA 用 `CODE_SIGNING_ALLOWED=NO` 构建，作用是确认源码在真实 Xcode/iPhone SDK 下能够编译，并提供未签名产物。**它不能直接安装到普通未越狱 iPhone。**要正常安装，仍需 Apple Developer 证书、与 `local.airferry.ios` 匹配的 provisioning profile，并在 GitHub Secrets 中安全配置签名材料；也可以下载源码后在自己的 Mac/Xcode 中选择 Personal Team 直接真机运行。不要把 `.p12`、证书密码或 provisioning profile 直接提交进仓库。

## 使用

### 发送

1. 打开“发送”，选择文件；单文件上限由共享协议限制为 256 MB。
2. 默认“高速 896B / 20fps + 单码”，优先保证普通相机拿到完整曝光帧。距离近、画面稳定时可以选择激进或双码；反光、抖动或对焦困难时切换“稳定 512B”。
3. 文件大于约 32 MB 时会自动分段。接收端提示某段完成后，手动切换下一段；因为光学单向链路没有回传通道，发送端无法自动得知接收进度。
4. 随时点击“返回重选”可停止播放并回到文件选择页。

### 接收

1. 打开“接收”并允许相机权限，将全部二维码放进白色取景框。
2. 单码和双码会自动识别；进度条来自 Rust 解码器的实际有效符号数，不把重复帧算作进度。
3. 大文件的分段先写入临时目录，全部到齐后按顺序合并，并执行大小、CRC32 和 SHA-256 校验；成功后临时数据会删除。
4. 完成后可直接预览视频、音频、图片、PDF 和系统支持的文档，或用分享面板保存到其他位置。

## 性能说明

二维码光学传输的速度仍远低于 Wi-Fi/数据线。默认 896B 单码在 20fps 且每帧都成功识别的理论载荷上限约 17.5 KB/s；激进 1400B 单码在 24fps 的理论上限约 32.8 KB/s。双码理论上翻倍，但二维码变小后丢帧率可能上升。实际应以接收端有效符号增长速度为准。RaptorQ 会持续发送新的修复符号，丢帧不会要求重传整个文件。
