//! Image-nine source-bound shared geometry, semantic colors and font masks.
use prism::{
    appearance::{Accent, Preferences, TextScale, Theme},
    palette::Palette,
    theme, App, Bounds, Canvas, PixelFormat, Shell,
};
use std::io::Write;
fn prefs(theme: &str, accent: &str) -> Preferences {
    Preferences {
        theme: match theme {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            "contrast" => Theme::HighContrast,
            _ => panic!("unknown theme"),
        },
        accent: match accent {
            "blue" => Accent::Blue,
            "violet" => Accent::Violet,
            "teal" => Accent::Teal,
            "orange" => Accent::Orange,
            _ => panic!("unknown accent"),
        },
        text_scale: TextScale::Percent100,
    }
}
fn bounds(name: &str, b: Bounds) {
    println!("{name} {} {} {} {}", b.x, b.y, b.width, b.height);
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args[1].as_str() {
        "geometry" => {
            let mut shell = Shell::new();
            shell.set_viewport(args[2].parse().unwrap(), args[3].parse().unwrap());
            for (name, index) in [
                ("home_button", 0),
                ("terminal_button", 1),
                ("files_button", 2),
                ("monitor_button", 3),
                ("settings_button", 4),
            ] {
                bounds(name, shell.taskbar_item_bounds(index).unwrap());
            }
            bounds("files", shell.desktop().window(App::Files).bounds);
            bounds("settings", shell.desktop().window(App::Settings).bounds);
            bounds("monitor", shell.desktop().window(App::Monitor).bounds);
            for (name, index) in [("file_create", 0), ("file_rename", 1), ("file_delete", 2)] {
                bounds(name, shell.file_action_button_bounds(index).unwrap());
            }
            let b = shell.file_dialog_bounds();
            bounds("dialog", b);
            bounds(
                "dialog_input",
                Bounds {
                    x: b.x + 20,
                    y: b.y + 88,
                    width: b.width - 40,
                    height: 40,
                },
            );
            bounds(
                "dialog_cancel",
                Bounds {
                    x: b.x + 20,
                    y: b.y + b.height - 60,
                    width: 112,
                    height: 36,
                },
            );
            bounds(
                "dialog_apply",
                Bounds {
                    x: b.x + 144,
                    y: b.y + b.height - 60,
                    width: 112,
                    height: 36,
                },
            );
            let files = shell.desktop().window(App::Files).bounds;
            for index in 0..9 {
                bounds(
                    &format!("file_row_{index}"),
                    Bounds {
                        x: files.x + 12,
                        y: files.y + 106 + index * 24,
                        width: files.width - 24,
                        height: 24,
                    },
                );
            }
            for group in 0..3 {
                for index in 0..if group == 1 { 4 } else { 3 } {
                    bounds(
                        &format!("appearance_{group}_{index}"),
                        shell.appearance_control_bounds(group, index).unwrap(),
                    );
                }
            }
        }
        "palette" => {
            let p = Palette::new(prefs(&args[2], &args[3]));
            for (name, color) in [
                ("surface", p.surface),
                ("raised", p.raised),
                ("background", p.background),
                ("text", p.text),
                ("muted", p.muted),
                ("accent", p.accent),
                ("error", p.error),
                ("warning", p.warning),
            ] {
                println!("{name} {color:06x}");
            }
        }
        "text" => {
            let p = Palette::new(prefs(&args[2], &args[3]));
            let style = &args[4];
            let text = &args[5];
            let mut pixels = vec![0; 600 * 32];
            let mut canvas = Canvas::new(&mut pixels, 600, 32, 600, PixelFormat::Bgr).unwrap();
            canvas.set_palette(p);
            let background = if style == "field" {
                theme::SURFACE_RAISED
            } else {
                theme::SURFACE
            };
            canvas.clear(background);
            let (font, color) = match style.as_str() {
                "title" => (theme::Font::Title, theme::TEXT),
                "error" => (theme::Font::Body, 0xb42338),
                "muted" => (theme::Font::Body, theme::MUTED),
                _ => (theme::Font::Body, theme::TEXT),
            };
            theme::text(&mut canvas, 0, 0, text, color, font);
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(b"P6\n600 32\n255\n").unwrap();
            for p in pixels {
                stdout
                    .write_all(&[(p >> 16) as u8, (p >> 8) as u8, p as u8])
                    .unwrap();
            }
        }
        _ => panic!("unknown helper operation"),
    }
}
