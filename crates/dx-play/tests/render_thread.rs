//! The render thread, in a test binary of its own (the display tests change the picture settings the process shares).
//! Set `DEUSEX_DIR` to the game folder.
use dx_play::Session;
use std::path::PathBuf;

const DT: f32 = 1.0 / 60.0;

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    Some(s)
}

/// The render thread (`dx_vk::render_thread`): frames handed to it are the frames drawn on one thread. The same game
/// state, ten frames each way, the last one read back; the thread ends cleanly and gives the renderer back.
#[test]
fn the_render_thread_draws_what_one_thread_draws() {
    let picture = |threaded: bool| -> Option<Vec<u8>> {
        let mut s = new_game()?;
        let renderer = s.make_vk_renderer(&[]).unwrap();
        let mut rt = dx_vk::render_thread::RenderThread::new(renderer, threaded);
        assert_eq!(rt.threaded(), threaded);
        let mut prep = Vec::new();
        for _ in 0..10 {
            s.render_vk_bench_thread(&mut rt, (640, 360), &mut prep).unwrap();
        }
        let (_, _, rgba) = rt.renderer().unwrap().read_last_frame().unwrap().unwrap();
        rt.into_inner().unwrap().destroy();
        Some(rgba)
    };
    let Some(one) = picture(false) else { return };
    let two = picture(true).unwrap();
    let differ = one.iter().zip(&two).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} bytes differ between the frame drawn on one thread and on the render thread");
}
