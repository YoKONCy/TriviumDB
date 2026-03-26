fn main() {
    // 仅在启用 nodejs feature 时初始化 napi-build
    // napi-build 负责生成链接 Node.js 所需的 linker flags
    #[cfg(feature = "nodejs")]
    napi_build::setup();
}
