//! The visibility pass of the original renderer (Render.dll `URender::OccludeBsp` and friends), see
//! `docs/render-visibility.md`. It decides which world surfaces, zones and actors the original would draw; only drawn
//! actors get `LastRenderTime` stamped, which scripts read through `LastRendered()`.

pub mod bound;
pub mod frame;
pub mod raster;
pub mod span;
pub mod dynbsp;
pub mod walk;

pub use frame::Frame;
pub use span::SpanBuffer;
