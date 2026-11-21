/*
 * kmscon - Terminal
 *
 * Copyright (c) 2011-2012 David Herrmann <dh.herrmann@googlemail.com>
 * Copyright (c) 2011 University of Tuebingen
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files
 * (the "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice shall be included
 * in all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
 * OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
 * CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
 * TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
 * SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
// Cached outlined arrow cursor; the hotspot follows the arrow tip.

pub struct Cursor {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    hotspot: (usize, usize),
}
impl Cursor {
    pub fn arrow(font_height: usize) -> Self {
        let height = font_height.clamp(24, 48);
        let width = (20 * height).div_ceil(27);
        let scale = height as f64 / 27.;
        // Full arrow silhouette, including its diagonal tail.
        let polygon = [
            (1., 1.),
            (1., 20.),
            (6., 15.),
            (10., 24.),
            (14., 22.),
            (10., 14.),
            (18., 14.),
        ];
        // Supersample both the fill and outline; the left polygon edge stays exactly vertical.
        const SAMPLES: usize = 8;
        let outline = (height / 24).max(1) as f64 / scale;
        let mut pixels = vec![0; width * height];
        for y in 0..height {
            for x in 0..width {
                let mut white = 0usize;
                let mut dark = 0usize;
                for sy in 0..SAMPLES {
                    for sx in 0..SAMPLES {
                        let px = (x as f64 + (sx as f64 + 0.5) / SAMPLES as f64) / scale;
                        let py = (y as f64 + (sy as f64 + 0.5) / SAMPLES as f64) / scale;
                        let mut inside = false;
                        let mut distance = f64::INFINITY;
                        for i in 0..polygon.len() {
                            let (ax, ay) = polygon[i];
                            let (bx, by) = polygon[(i + 1) % polygon.len()];
                            if (ay > py) != (by > py) && px < (bx - ax) * (py - ay) / (by - ay) + ax
                            {
                                inside = !inside;
                            }
                            let dx = bx - ax;
                            let dy = by - ay;
                            let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy))
                                .clamp(0., 1.);
                            distance = distance
                                .min((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2));
                        }
                        if inside {
                            white += 1;
                        } else if distance <= outline * outline {
                            dark += 1;
                        }
                    }
                }
                let covered = white + dark;
                if covered == 0 {
                    continue;
                }
                let alpha = ((covered * 255 + SAMPLES * SAMPLES / 2) / (SAMPLES * SAMPLES)) as u32;
                let level = ((white * 255 + dark * 16) / covered) as u32;
                pixels[y * width + x] = (alpha << 24) | (level << 16) | (level << 8) | level;
            }
        }
        Self {
            width,
            height,
            pixels,
            hotspot: (scale.round() as usize, scale.round() as usize),
        }
    }
    pub fn draw(&self, pixels: &mut [u32], stride: usize, position: (usize, usize)) {
        let left = position.0 as isize - self.hotspot.0 as isize;
        let top = position.1 as isize - self.hotspot.1 as isize;
        for y in 0..self.height {
            for x in 0..self.width {
                let px = left + x as isize;
                let py = top + y as isize;
                if px < 0
                    || py < 0
                    || px >= stride as isize
                    || py >= (pixels.len() / stride) as isize
                {
                    continue;
                }
                let color = self.pixels[y * self.width + x];
                let alpha = color >> 24;
                if alpha == 0 {
                    continue;
                }
                let pixel = &mut pixels[py as usize * stride + px as usize];
                let mut mixed = 0;
                for shift in [0, 8, 16] {
                    mixed |= ((((color >> shift) & 255) * alpha
                        + ((*pixel >> shift) & 255) * (255 - alpha))
                        / 255)
                        << shift;
                }
                *pixel = mixed;
            }
        }
    }
}
