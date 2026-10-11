//! Exact public geometry replay only; no host picture substitutes guest pixels.
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
    let pid = a[5].parse().unwrap();
    let config: u64 = a[11].parse().unwrap();
    let pw = n(12) as usize;
    let ph = n(13) as usize;
    let mut s = Shell::new();
    s.set_viewport(n(1) as usize, n(2) as usize);
    s.submit_client_surface_configured(
        id.slot,
        id.generation,
        1,
        1,
        pid,
        &a[6],
        64,
        48,
        64,
        PixelFormat::Bgr,
        &[0; 64 * 48 * 4],
    )
    .unwrap();
    let b = s.native_window_presentation(id).unwrap().bounds;
    let title = s.native_window_titlebar_bounds(id).unwrap();
    let x = title.x + title.width / 2;
    let y = title.y + title.height / 2;
    s.on_pointer_packet(x, y, 1);
    s.on_pointer_packet(x + n(7) - b.x, y + n(8) - b.y, 1);
    s.on_pointer_packet(x + n(7) - b.x, y + n(8) - b.y, 0);
    if config > 1 {
        let b = s.native_window_presentation(id).unwrap().bounds;
        let h = s.native_window_resize_handle_bounds(id).unwrap();
        let x = h.x + h.width / 2;
        let y = h.y + h.height / 2;
        s.on_pointer_packet(x, y, 1);
        s.on_pointer_packet(x + n(9) - b.width, y + n(10) - b.height, 1);
        s.on_pointer_packet(x + n(9) - b.width, y + n(10) - b.height, 0);
        let action = s.take_native_resize_action().unwrap();
        assert!(s.native_resize_result(action.request, Ok(config)));
        let pixels = vec![0; pw * ph * 4];
        s.submit_client_surface_configured(
            id.slot,
            id.generation,
            2,
            config,
            pid,
            &a[6],
            pw,
            ph,
            pw,
            PixelFormat::Bgr,
            &pixels,
        )
        .unwrap();
    }
    let b = s.native_window_presentation(id).unwrap().bounds;
    assert_eq!((b.x, b.y, b.width, b.height), (n(7), n(8), n(9), n(10)));
    emit("card", b);
    emit("title", s.native_window_titlebar_bounds(id).unwrap());
    emit("content", s.client_content_bounds(id.slot).unwrap());
    emit("resize", s.native_window_resize_handle_bounds(id).unwrap());
    for (name, index) in [("home", 0), ("terminal", 1), ("files", 2), ("settings", 4)] {
        emit(name, s.taskbar_item_bounds(index).unwrap());
    }
}
