// How a display component is shown: its VNC port, and the rotation and
// scaling from the component's pixels to the picture a viewer gets.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScreenOpts {
    // None: the first free port from 5900 up.
    pub port: Option<u16>,
    pub scale: usize,
    // Degrees clockwise: 0, 90, 180 or 270.
    pub rotate: u16
}

impl Default for ScreenOpts {
    fn default() -> Self {
        Self { port: None, scale: 4, rotate: 0 }
    }
}

impl ScreenOpts {
    // Size of the picture for a display of width x height pixels.
    pub fn out_size(&self, width: usize, height: usize) -> (usize, usize) {
        let (w, h) = if self.rotate % 180 == 0 { (width, height) } else { (height, width) };
        (w * self.scale, h * self.scale)
    }

    // Rotates and scales src (width x height, row-major) into dst, which
    // has out_size(width, height) pixels.
    pub fn transform(&self, src: &[u32], width: usize, height: usize, dst: &mut [u32]) {
        let (out_w, out_h) = self.out_size(width, height);
        let (w, h) = (out_w / self.scale, out_h / self.scale);
        for y in 0..h {
            for x in 0..w {
                // The source pixel shown at (x, y) of the rotated picture.
                let (sx, sy) = match self.rotate {
                    90 => (y, height - 1 - x),
                    180 => (width - 1 - x, height - 1 - y),
                    270 => (width - 1 - y, x),
                    _ => (x, y)
                };
                let pixel = src[sy * width + sx];
                for dy in 0..self.scale {
                    let row = (y * self.scale + dy) * out_w + x * self.scale;
                    dst[row..row + self.scale].fill(pixel);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 3x2 picture:  1 2 3
    //                 4 5 6
    const SRC: [u32; 6] = [1, 2, 3, 4, 5, 6];

    fn turn(rotate: u16, scale: usize) -> (usize, usize, Vec<u32>) {
        let opts = ScreenOpts { port: None, scale, rotate };
        let (w, h) = opts.out_size(3, 2);
        let mut dst = vec![0; w * h];
        opts.transform(&SRC, 3, 2, &mut dst);
        (w, h, dst)
    }

    #[test]
    fn rotates_clockwise() {
        assert_eq!(turn(0, 1), (3, 2, vec![1, 2, 3, 4, 5, 6]));
        assert_eq!(turn(90, 1), (2, 3, vec![4, 1, 5, 2, 6, 3]));
        assert_eq!(turn(180, 1), (3, 2, vec![6, 5, 4, 3, 2, 1]));
        assert_eq!(turn(270, 1), (2, 3, vec![3, 6, 2, 5, 1, 4]));
    }

    #[test]
    fn scales() {
        assert_eq!(turn(90, 2), (4, 6, vec![4, 4, 1, 1, 4, 4, 1, 1, 5, 5, 2, 2, 5, 5, 2, 2, 6, 6, 3, 3, 6, 6, 3, 3]));
    }
}
