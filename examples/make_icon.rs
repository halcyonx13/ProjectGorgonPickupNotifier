//! Regenerates res/app.ico: cargo run --example make_icon
fn main() {
    let ico = pg_pickup_notifier::appicon::ico_file(&[16, 20, 24, 32, 40, 48, 64, 256]);
    std::fs::write("res/app.ico", ico).unwrap();
    let img = image::RgbaImage::from_raw(256, 256, pg_pickup_notifier::appicon::rgba(256)).unwrap();
    img.save("res/app-256.png").unwrap();
}
