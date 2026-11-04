#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    from: [f32; 2],
    to: [f32; 2],
    start: f32,
    length: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polyline {
    segments: Vec<Segment>,
    total: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub point: [f32; 2],
    pub angle: f32,
}

struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn skip_separators(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b',')
        {
            self.at += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip_separators();
        let byte = *self.bytes.get(self.at)?;
        byte.is_ascii_alphabetic().then(|| {
            self.at += 1;
            byte
        })
    }

    fn has_number(&mut self) -> bool {
        self.skip_separators();
        self.bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let start = self.at;
        let mut seen_dot = false;
        let mut seen_exp = false;
        while let Some(&byte) = self.bytes.get(self.at) {
            let accept = match byte {
                b'0'..=b'9' => true,
                b'-' | b'+' => {
                    self.at == start
                        || matches!(self.bytes[self.at - 1], b'e' | b'E')
                }
                b'.' if !seen_dot && !seen_exp => {
                    seen_dot = true;
                    true
                }
                b'e' | b'E' if !seen_exp && self.at > start => {
                    seen_exp = true;
                    true
                }
                _ => false,
            };
            if !accept {
                break;
            }
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()?
            .parse()
            .ok()
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip_separators();
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        match byte {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }

    fn point(&mut self) -> Option<[f32; 2]> {
        Some([self.number()?, self.number()?])
    }
}

struct Builder {
    segments: Vec<Segment>,
    total: f32,
}

impl Builder {
    fn line(&mut self, from: [f32; 2], to: [f32; 2]) {
        let length = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
        if length > 1.0e-6 {
            self.segments.push(Segment {
                from,
                to,
                start: self.total,
                length,
            });
            self.total += length;
        }
    }

    fn curve(&mut self, from: [f32; 2], eval: impl Fn(f32) -> [f32; 2], hint: f32) {
        let steps = (hint / 2.0).ceil().clamp(6.0, 96.0) as u32;
        let mut previous = from;
        for step in 1..=steps {
            let next = eval(step as f32 / steps as f32);
            self.line(previous, next);
            previous = next;
        }
    }

    fn cubic(&mut self, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]) {
        let hint = distance(p0, p1) + distance(p1, p2) + distance(p2, p3);
        self.curve(
            p0,
            |t| {
                let u = 1.0 - t;
                let blend = |i: usize| {
                    u * u * u * p0[i]
                        + 3.0 * u * u * t * p1[i]
                        + 3.0 * u * t * t * p2[i]
                        + t * t * t * p3[i]
                };
                [blend(0), blend(1)]
            },
            hint,
        );
    }

    fn quad(&mut self, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]) {
        let hint = distance(p0, p1) + distance(p1, p2);
        self.curve(
            p0,
            |t| {
                let u = 1.0 - t;
                let blend = |i: usize| u * u * p0[i] + 2.0 * u * t * p1[i] + t * t * p2[i];
                [blend(0), blend(1)]
            },
            hint,
        );
    }

    fn arc(
        &mut self,
        from: [f32; 2],
        radii: [f32; 2],
        rotation_degrees: f32,
        large: bool,
        sweep: bool,
        to: [f32; 2],
    ) {
        let (mut rx, mut ry) = (radii[0].abs(), radii[1].abs());
        if rx < 1.0e-6 || ry < 1.0e-6 || from == to {
            self.line(from, to);
            return;
        }
        let phi = rotation_degrees.to_radians();
        let (sin, cos) = phi.sin_cos();
        let dx = (from[0] - to[0]) / 2.0;
        let dy = (from[1] - to[1]) / 2.0;
        let x1 = cos * dx + sin * dy;
        let y1 = -sin * dx + cos * dy;
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if lambda > 1.0 {
            let scale = lambda.sqrt();
            rx *= scale;
            ry *= scale;
        }
        let numerator = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
        let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let sign = if large == sweep { -1.0 } else { 1.0 };
        let factor = sign * (numerator / denominator).max(0.0).sqrt();
        let cx1 = factor * rx * y1 / ry;
        let cy1 = -factor * ry * x1 / rx;
        let cx = cos * cx1 - sin * cy1 + (from[0] + to[0]) / 2.0;
        let cy = sin * cx1 + cos * cy1 + (from[1] + to[1]) / 2.0;
        let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
            let dot = ux * vx + uy * vy;
            let length = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
            let value = (dot / length).clamp(-1.0, 1.0).acos();
            if ux * vy - uy * vx < 0.0 {
                -value
            } else {
                value
            }
        };
        let theta = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
        let mut delta = angle(
            (x1 - cx1) / rx,
            (y1 - cy1) / ry,
            (-x1 - cx1) / rx,
            (-y1 - cy1) / ry,
        );
        if !sweep && delta > 0.0 {
            delta -= std::f32::consts::TAU;
        }
        if sweep && delta < 0.0 {
            delta += std::f32::consts::TAU;
        }
        let hint = (rx + ry) * delta.abs();
        self.curve(
            from,
            |t| {
                let a = theta + delta * t;
                let (sa, ca) = a.sin_cos();
                [
                    cos * rx * ca - sin * ry * sa + cx,
                    sin * rx * ca + cos * ry * sa + cy,
                ]
            },
            hint,
        );
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn reflect(control: [f32; 2], about: [f32; 2]) -> [f32; 2] {
    [2.0 * about[0] - control[0], 2.0 * about[1] - control[1]]
}

impl Polyline {
    pub fn parse(data: &str) -> Option<Self> {
        let mut scanner = Scanner {
            bytes: data.as_bytes(),
            at: 0,
        };
        let mut builder = Builder {
            segments: Vec::new(),
            total: 0.0,
        };
        let mut current = [0.0f32; 2];
        let mut subpath_start = [0.0f32; 2];
        let mut last_cubic: Option<[f32; 2]> = None;
        let mut last_quad: Option<[f32; 2]> = None;
        let mut command = scanner.command()?;
        if !matches!(command, b'M' | b'm') {
            return None;
        }
        loop {
            let relative = command.is_ascii_lowercase();
            let origin = if relative { current } else { [0.0, 0.0] };
            let at = |p: [f32; 2]| [origin[0] + p[0], origin[1] + p[1]];
            let mut next_cubic = None;
            let mut next_quad = None;
            match command.to_ascii_uppercase() {
                b'M' => {
                    current = at(scanner.point()?);
                    subpath_start = current;
                    command = if relative { b'l' } else { b'L' };
                }
                b'L' => {
                    let to = at(scanner.point()?);
                    builder.line(current, to);
                    current = to;
                }
                b'H' => {
                    let x = scanner.number()? + if relative { current[0] } else { 0.0 };
                    let to = [x, current[1]];
                    builder.line(current, to);
                    current = to;
                }
                b'V' => {
                    let y = scanner.number()? + if relative { current[1] } else { 0.0 };
                    let to = [current[0], y];
                    builder.line(current, to);
                    current = to;
                }
                b'C' => {
                    let (c1, c2, to) = (
                        at(scanner.point()?),
                        at(scanner.point()?),
                        at(scanner.point()?),
                    );
                    builder.cubic(current, c1, c2, to);
                    next_cubic = Some(c2);
                    current = to;
                }
                b'S' => {
                    let (c2, to) = (at(scanner.point()?), at(scanner.point()?));
                    let c1 = last_cubic.map_or(current, |c| reflect(c, current));
                    builder.cubic(current, c1, c2, to);
                    next_cubic = Some(c2);
                    current = to;
                }
                b'Q' => {
                    let (c, to) = (at(scanner.point()?), at(scanner.point()?));
                    builder.quad(current, c, to);
                    next_quad = Some(c);
                    current = to;
                }
                b'T' => {
                    let to = at(scanner.point()?);
                    let c = last_quad.map_or(current, |c| reflect(c, current));
                    builder.quad(current, c, to);
                    next_quad = Some(c);
                    current = to;
                }
                b'A' => {
                    let radii = scanner.point()?;
                    let rotation = scanner.number()?;
                    let large = scanner.flag()?;
                    let sweep = scanner.flag()?;
                    let to = at(scanner.point()?);
                    builder.arc(current, radii, rotation, large, sweep, to);
                    current = to;
                }
                b'Z' => {
                    builder.line(current, subpath_start);
                    current = subpath_start;
                }
                _ => return None,
            }
            last_cubic = next_cubic;
            last_quad = next_quad;
            if !matches!(command.to_ascii_uppercase(), b'Z') && scanner.has_number() {
                continue;
            }
            match scanner.command() {
                Some(next) => command = next,
                None => break,
            }
        }
        (builder.total > 0.0).then_some(Self {
            segments: builder.segments,
            total: builder.total,
        })
    }

    pub fn length(&self) -> f32 {
        self.total
    }

    pub fn sample(&self, distance: f32) -> Option<Sample> {
        (distance >= 0.0 && distance <= self.total).then_some(())?;
        let at = self
            .segments
            .partition_point(|segment| segment.start + segment.length < distance)
            .min(self.segments.len() - 1);
        let segment = self.segments[at];
        let t = ((distance - segment.start) / segment.length).clamp(0.0, 1.0);
        let dx = segment.to[0] - segment.from[0];
        let dy = segment.to[1] - segment.from[1];
        Some(Sample {
            point: [segment.from[0] + dx * t, segment.from[1] + dy * t],
            angle: dy.atan2(dx),
        })
    }
}
