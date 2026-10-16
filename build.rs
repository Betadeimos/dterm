// Links the exe icon. dterm.res is prebuilt from dterm.rc + assets/icon.ico; to change the icon run:
//   rc.exe /fo dterm.res dterm.rc   (rc.exe ships with the Windows SDK)
fn main() {
    println!("cargo:rerun-if-changed=dterm.res");
    println!("cargo:rustc-link-arg-bins=dterm.res");
}
