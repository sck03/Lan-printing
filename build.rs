fn main() {
    println!("cargo:rerun-if-changed=assets/lan-print.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/lan-print.ico")
            .set("ProductName", "局域打印站 LanPrint")
            .set("FileDescription", "局域打印站 — 打印与扫描")
            .compile()
            .expect("failed to compile Windows icon resources");
    }
}
