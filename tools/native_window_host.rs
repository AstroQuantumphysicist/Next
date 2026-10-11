//! Source-bound geometry only; it never substitutes pixels for a guest frame.
use prism::{client_interaction::SurfaceRef, Bounds, PixelFormat, Shell};
fn emit(name: &str, b: Bounds) {
    println!("{name} {} {} {} {}", b.x, b.y, b.width, b.height);
}
fn main() {
    let a: Vec<_> = std::env::args().collect();
    let n = |i: usize| a[i].parse::<i32>().unwrap();
    let id = SurfaceRef {
        slot: n(3) as usize,
        generation: a[4].parse().unwrap(),
    };
    let mut shell = Shell::new();
    shell.set_viewport(n(1) as usize, n(2) as usize);
    shell
        .submit_client_surface(
            id.slot,
            id.generation,
            1,
            a[5].parse().unwrap(),
            &a[6],
            64,
            48,
            64,
            PixelFormat::Bgr,
            &[0; 64 * 48 * 4],
        )
        .unwrap();
    let before = shell.native_window_presentation(id).unwrap().bounds;
    if a.len() == 7 {
        emit("card", before);
        emit("title", shell.native_window_titlebar_bounds(id).unwrap());
        emit("content", shell.client_content_bounds(id.slot).unwrap());
        emit(
            "minimize",
            shell.native_window_minimize_button_bounds(id).unwrap(),
        );
        emit(
            "restore",
            shell.native_window_restore_button_bounds(id.slot).unwrap(),
        );
        return;
    }
    let title = shell.native_window_titlebar_bounds(id).unwrap();
    let x = title.x + title.width / 2;
    let y = title.y + title.height / 2;
    shell.on_pointer_packet(x, y, 1);
    shell.on_pointer_packet(x + n(7) - before.x, y + n(8) - before.y, 1);
    shell.on_pointer_packet(x + n(7) - before.x, y + n(8) - before.y, 0);
    let card = shell.native_window_presentation(id).unwrap().bounds;
    assert_eq!(
        (card.x, card.y, card.width, card.height),
        (n(7), n(8), n(9), n(10))
    );
    emit("card", card);
    emit("title", shell.native_window_titlebar_bounds(id).unwrap());
    emit("content", shell.client_content_bounds(id.slot).unwrap());
    emit(
        "minimize",
        shell.native_window_minimize_button_bounds(id).unwrap(),
    );
    emit(
        "restore",
        shell.native_window_restore_button_bounds(id.slot).unwrap(),
    );
}
