use egui::{ColorImage, Context, TextureHandle, TextureOptions};

pub struct DecodedFrame {
    pub image: ColorImage,
    pub pts: f64,
}

pub struct FramePresenter {
    texture: Option<TextureHandle>,
    seq: u64,
}

impl FramePresenter {
    pub fn new() -> Self {
        Self {
            texture: None,
            seq: 0,
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    pub fn show(&mut self, ctx: &Context, frame: DecodedFrame) {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.texture = Some(ctx.load_texture(
            format!("video_frame_{seq}"),
            frame.image,
            TextureOptions::LINEAR,
        ));
    }
}
