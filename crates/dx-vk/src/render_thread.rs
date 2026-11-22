//! The renderer on a thread of its own (deusex_shin docs/RENDER_THREAD.md, phase 3). The main thread keeps everything
//! that reads the map or the game: it builds the frame (`FramePacket`, applied to the renderer) and does the first half
//! of the frame (`Renderer::begin_window_frame`: the slot waited for, textures and meshes loaded on first use, the
//! procedural textures and light maps written). Then it hands the frame over here and goes on with the next game step,
//! while this thread does the second half (`Renderer::finish_window_frame`: the swapchain image, where VSync waits, the
//! recording, the submission and the presentation).
//!
//! The renderer sits behind a lock, and the main thread uses it only between frames: `renderer()` and `submit` first
//! wait for the frame handed over last. So the two halves of each frame run in order, as on one thread, and the queue is
//! never used by both threads at once. At most one frame is in hand at a time.
use crate::renderer::Renderer;
use dx_soft::{ActorDraw, Camera};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};

/// The second half of one frame, as `finish_window_frame` takes it.
pub struct Job {
    pub cam: Camera,
    pub actors: Vec<ActorDraw>,
    pub brightness: f32,
    pub crosshair: bool,
    pub window_size: (u32, u32),
    /// An offscreen frame (`Renderer::finish_queued_frame`, the bench) instead of the window's.
    pub offscreen: bool,
}

/// The second half of `job`'s frame.
fn finish(r: &mut Renderer, job: &Job) -> Result<bool, String> {
    if job.offscreen {
        r.finish_queued_frame(&job.cam, &job.actors).map(|_| true).map_err(|e| e.to_string())
    } else {
        r.finish_window_frame(&job.cam, &job.actors, job.brightness, job.crosshair, job.window_size).map_err(|e| e.to_string())
    }
}

pub struct RenderThread {
    shared: Arc<Mutex<Renderer>>,
    jobs: Option<SyncSender<Job>>,
    done: Receiver<Result<bool, String>>,
    pending: bool,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Whether the last frame finished was drawn (false while the swapchain is being made).
    pub last_drawn: bool,
    /// The renderer's `render_size`, kept here so the main thread can read it while a frame is in hand.
    render_size: Option<(u32, u32)>,
}

impl RenderThread {
    /// `threaded` false: no thread, `submit` does the second half at once on the caller's thread (as before the
    /// render thread; for comparisons and as a fallback).
    pub fn new(renderer: Renderer, threaded: bool) -> RenderThread {
        let render_size = renderer.render_size;
        let shared = Arc::new(Mutex::new(renderer));
        if !threaded {
            let (_, done) = sync_channel::<Result<bool, String>>(1);
            return RenderThread { shared, jobs: None, done, pending: false, handle: None, last_drawn: false, render_size };
        }
        let (jobs, inbox) = sync_channel::<Job>(1);
        let (outbox, done) = sync_channel::<Result<bool, String>>(1);
        let s = shared.clone();
        let handle = std::thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                while let Ok(job) = inbox.recv() {
                    let result = {
                        let mut r = s.lock().unwrap_or_else(|e| e.into_inner());
                        finish(&mut r, &job)
                    };
                    if outbox.send(result).is_err() {
                        break;
                    }
                }
            })
            .expect("render thread");
        RenderThread { shared, jobs: Some(jobs), done, pending: false, handle: Some(handle), last_drawn: false, render_size }
    }

    /// Waits for the frame handed over last (if any) and reports its error.
    pub fn wait(&mut self) -> Result<(), String> {
        if self.pending {
            self.pending = false;
            match self.done.recv() {
                Ok(r) => self.last_drawn = r?,
                Err(_) => return Err("the render thread stopped".into()),
            }
        }
        Ok(())
    }

    /// The renderer, for the main thread's part of a frame and anything else between frames (resizes, settings, a
    /// new map, reading a frame back): after the frame in hand is done.
    pub fn renderer(&mut self) -> Result<MutexGuard<'_, Renderer>, String> {
        self.wait()?;
        Ok(self.shared.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Hands the second half of a frame (after `begin_window_frame` returned true) to the thread.
    pub fn submit(&mut self, job: Job) -> Result<(), String> {
        self.wait()?;
        let Some(jobs) = self.jobs.as_ref() else {
            let mut r = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            self.last_drawn = finish(&mut r, &job)?;
            return Ok(());
        };
        jobs.send(job).map_err(|_| "the render thread stopped".to_string())?;
        self.pending = true;
        Ok(())
    }

    /// The resolution the game renders at in the window (`Renderer::render_size`).
    pub fn render_size(&self) -> Option<(u32, u32)> {
        self.render_size
    }

    pub fn set_render_size(&mut self, size: Option<(u32, u32)>) -> Result<(), String> {
        self.renderer()?.render_size = size;
        self.render_size = size;
        Ok(())
    }

    /// Whether the second half runs on a thread of its own.
    pub fn threaded(&self) -> bool {
        self.handle.is_some()
    }

    /// Ends the thread (after the frame in hand) and gives the renderer back (to be destroyed).
    pub fn into_inner(mut self) -> Result<Renderer, String> {
        let waited = self.wait();
        let RenderThread { shared, jobs, handle, .. } = self;
        // the thread ends when its inbox closes
        drop(jobs);
        if let Some(h) = handle {
            let _ = h.join();
        }
        waited?;
        Arc::try_unwrap(shared).map_err(|_| "the renderer is still in use".to_string()).map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()))
    }
}
