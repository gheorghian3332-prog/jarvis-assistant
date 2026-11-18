use crate::error::{Result, VisionError};

/// A picture that lives only in memory: top-down BGRA, 4 bytes per pixel, alpha always 255.
#[derive(Clone)]
pub struct Frame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print pixels.
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        if width == 0 || height == 0 || expected != Some(bgra.len()) {
            return Err(VisionError::Image(format!(
                "invalid frame {width}x{height} with {} bytes",
                bgra.len()
            )));
        }
        Ok(Self {
            width,
            height,
            bgra,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// True when every colour byte is zero: what GDI returns for a locked or switched-off screen.
    pub fn is_blank(&self) -> bool {
        self.bgra
            .chunks_exact(4)
            .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0)
    }

    /// Copy of the sub-rectangle `x, y, w, h` (clamped to the frame). `None` if it is empty.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Option<Frame> {
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        if x >= x1 || y >= y1 {
            return None;
        }
        let mut out = Vec::with_capacity(((x1 - x) * (y1 - y)) as usize * 4);
        for row in y..y1 {
            let start = (row as usize * self.width as usize + x as usize) * 4;
            out.extend_from_slice(&self.bgra[start..start + (x1 - x) as usize * 4]);
        }
        Frame::new(x1 - x, y1 - y, out).ok()
    }

    /// Shrinks so that the larger side is at most `max_side` (area averaging, keeps the aspect
    /// ratio). A smaller frame is returned unchanged.
    pub fn scaled_to_max(&self, max_side: u32) -> Frame {
        let longest = self.width.max(self.height);
        if max_side == 0 || longest <= max_side {
            return self.clone();
        }
        let ratio = f64::from(max_side) / f64::from(longest);
        let new_w = ((f64::from(self.width) * ratio).round() as u32).max(1);
        let new_h = ((f64::from(self.height) * ratio).round() as u32).max(1);
        let mut out = vec![0u8; new_w as usize * new_h as usize * 4];
        for dy in 0..new_h {
            let sy0 = (u64::from(dy) * u64::from(self.height) / u64::from(new_h)) as u32;
            let sy1 = ((u64::from(dy + 1) * u64::from(self.height)).div_ceil(u64::from(new_h))
                as u32)
                .clamp(sy0 + 1, self.height);
            for dx in 0..new_w {
                let sx0 = (u64::from(dx) * u64::from(self.width) / u64::from(new_w)) as u32;
                let sx1 = ((u64::from(dx + 1) * u64::from(self.width)).div_ceil(u64::from(new_w))
                    as u32)
                    .clamp(sx0 + 1, self.width);
                let mut sum = [0u64; 3];
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let at = (sy as usize * self.width as usize + sx as usize) * 4;
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += u64::from(self.bgra[at + channel]);
                        }
                    }
                }
                let count = u64::from(sx1 - sx0) * u64::from(sy1 - sy0);
                let at = (dy as usize * new_w as usize + dx as usize) * 4;
                for (channel, total) in sum.iter().enumerate() {
                    out[at + channel] = ((total + count / 2) / count) as u8;
                }
                out[at + 3] = 255;
            }
        }
        Frame {
            width: new_w,
            height: new_h,
            bgra: out,
        }
    }

    /// PNG bytes (RGB, no alpha), in memory.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut rgb = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for pixel in self.bgra.chunks_exact(4) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|error| VisionError::Image(error.to_string()))?;
        writer
            .write_image_data(&rgb)
            .map_err(|error| VisionError::Image(error.to_string()))?;
        writer
            .finish()
            .map_err(|error| VisionError::Image(error.to_string()))?;
        Ok(out)
    }

    /// Decodes a PNG (any colour type) from memory.
    pub fn from_png(bytes: &[u8]) -> Result<Frame> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder
            .read_info()
            .map_err(|error| VisionError::Image(error.to_string()))?;
        let mut buffer = vec![
            0u8;
            reader
                .output_buffer_size()
                .ok_or_else(|| VisionError::Image("PNG is too large".into()))?
        ];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| VisionError::Image(error.to_string()))?;
        let data = &buffer[..info.buffer_size()];
        let mut bgra = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        // Transparent pixels are composited on white so they do not turn black.
        let blend = |color: u8, alpha: u32| -> u8 {
            ((u32::from(color) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
        };
        match info.color_type {
            png::ColorType::Rgb => {
                for p in data.chunks_exact(3) {
                    bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            png::ColorType::Rgba => {
                for p in data.chunks_exact(4) {
                    let a = u32::from(p[3]);
                    bgra.extend_from_slice(&[blend(p[2], a), blend(p[1], a), blend(p[0], a), 255]);
                }
            }
            png::ColorType::Grayscale => {
                for &g in data {
                    bgra.extend_from_slice(&[g, g, g, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for p in data.chunks_exact(2) {
                    let g = blend(p[0], u32::from(p[1]));
                    bgra.extend_from_slice(&[g, g, g, 255]);
                }
            }
            png::ColorType::Indexed => {
                return Err(VisionError::Image("unexpanded indexed PNG".into()));
            }
        }
        Frame::new(info.width, info.height, bgra)
    }
}
