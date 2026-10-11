//! Host probe using the exact shared Prism geometry and font rasterizer.
use std::io::Write;
use prism::{Canvas, PixelFormat, Shell, theme, Key, notifications::{Source, Priority}};
fn rendered(shell: &Shell) -> Vec<u32> {
    let mut pixels=vec![0;1280*800];
    shell.render(&mut Canvas::new(&mut pixels,1280,800,1280,PixelFormat::Bgr).unwrap());
    pixels
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args[1] == "barrier-audit" {
        let mut shell=Shell::new(); shell.set_viewport(1280,800);
        let before=rendered(&shell); shell.on_key(Key::Left); assert_eq!(before,rendered(&shell));
        shell.publish_notification(Source::Process{pid:1},"org.next.fixture","Exit","Actual result",Priority::Warning,0).unwrap();
        shell.on_key(Key::Notifications);
        let before=rendered(&shell); shell.on_key(Key::Left); assert_eq!(before,rendered(&shell));
        shell.on_key(Key::Notifications);
        shell.show_file_portal(1,1,"org.next.fixture",false);
        let before=rendered(&shell); shell.on_key(Key::Notifications); shell.on_key(Key::Menu);
        assert_eq!(before,rendered(&shell));
        shell.on_key(Key::Left); assert_ne!(before,rendered(&shell));
        println!("Frozen Prism barriers PASS: empty Terminal Left and notification Left no-op; portal F8/Menu no-op; portal Left changes approval and is excluded");
    } else if args[1] == "geometry" {
        let mut shell = Shell::new();
        shell.set_viewport(args[2].parse().unwrap(), args[3].parse().unwrap());
        for (name, bounds) in [
            ("home_button", shell.taskbar_item_bounds(0).unwrap()),
            ("terminal_button", shell.taskbar_item_bounds(1).unwrap()),
            ("files_button", shell.taskbar_item_bounds(2).unwrap()),
            ("settings_button", shell.taskbar_item_bounds(4).unwrap()),
            ("home", shell.home_bounds()),
            ("home_files", shell.home_action_bounds(1).unwrap()),
            ("notices", shell.notification_bounds()),
        ] {
            println!("{name} {} {} {} {}", bounds.x, bounds.y, bounds.width, bounds.height);
        }
        shell.show_file_portal(1, 1, "org.next.fixture", false);
        let b = shell.file_portal_bounds().unwrap();
        println!("portal {} {} {} {}", b.x, b.y, b.width, b.height);
    } else {
        let mut pixels = vec![0; 400 * 32];
        let mut canvas = Canvas::new(&mut pixels, 400, 32, 400, PixelFormat::Bgr).unwrap();
        canvas.clear(if args[1] == "muted_selected" { theme::ACCENT_SOFT } else { theme::SURFACE });
        let (font, color) = match args[1].as_str() {
            "title" => (theme::Font::Title, theme::TEXT),
            "muted" | "muted_selected" => (theme::Font::Body, theme::MUTED),
            _ => (theme::Font::Body, theme::TEXT),
        };
        theme::text(&mut canvas, 0, 0, &args[2], color, font);
        let mut output = std::io::stdout().lock();
        output.write_all(b"P6\n400 32\n255\n").unwrap();
        for pixel in pixels {
            output.write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8]).unwrap();
        }
    }
}
