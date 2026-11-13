//! Homogeneous clipping, perspective interpolation and triangle rasterization.
use super::*;
const MAX_WORK: usize = 32 * 1024 * 1024;
#[derive(Clone)]
struct Vertex {
    position: [f64; 4],
    varyings: Vec<f64>,
}
impl Vertex {
    fn lerp(&self, b: &Self, t: f64) -> Self {
        let mut position = [0.; 4];
        for (i, p) in position.iter_mut().enumerate() {
            *p = self.position[i] + (b.position[i] - self.position[i]) * t;
        }
        Self {
            position,
            varyings: self
                .varyings
                .iter()
                .zip(&b.varyings)
                .map(|(a, b)| a + (b - a) * t)
                .collect(),
        }
    }
}
struct Screen {
    position: [f64; 3],
    inverse_w: f64,
    varyings: Vec<f64>,
}
struct Triangle {
    vertices: [Screen; 3],
    bounds: [i64; 4],
    area: f64,
}
fn edge(a: &Screen, b: &Screen, x: f64, y: f64) -> f64 {
    (b.position[0] - a.position[0]) * (y - a.position[1])
        - (b.position[1] - a.position[1]) * (x - a.position[0])
}
fn inclusive(a: &Screen, b: &Screen) -> bool {
    let (dx, dy) = (b.position[0] - a.position[0], b.position[1] - a.position[1]);
    dy < 0. || (dy == 0. && dx < 0.)
}
fn distance(v: &Vertex, plane: usize) -> f64 {
    let axis = plane / 2;
    v.position[3]
        + if plane % 2 == 0 {
            v.position[axis]
        } else {
            -v.position[axis]
        }
}
fn clip(vertices: [Vertex; 3]) -> Vec<Vertex> {
    let mut polygon = vertices.to_vec();
    for plane in 0..6 {
        if polygon.is_empty() {
            break;
        }
        let mut out = Vec::new();
        let mut a = polygon.last().unwrap();
        let mut da = distance(a, plane);
        for b in &polygon {
            let db = distance(b, plane);
            if (da >= 0.) != (db >= 0.) {
                let t = da / (da - db);
                out.push(a.lerp(b, t));
            }
            if db >= 0. {
                out.push(b.clone());
            }
            a = b;
            da = db;
        }
        polygon = out;
    }
    polygon
}
impl Context {
    pub(super) fn draw(&mut self, mode: u32, indices: &[u32]) -> Result<()> {
        let executable = self
            .programs
            .get(&self.current_program)
            .and_then(|p| p.executable.as_ref())
            .ok_or(Error::InvalidOperation)?
            .clone();
        self.validate_arrays(indices.iter().copied().max())?;
        self.validate_samplers(&executable)?;
        if indices.len() < 3 {
            return Ok(());
        }
        let mut uniforms = BTreeMap::new();
        for u in &executable.uniforms {
            uniforms.insert(u.name.clone(), u.value);
        }
        let mut vertices = Vec::with_capacity(indices.len());
        for &index in indices {
            let mut env = uniforms.clone();
            for (name, &location) in &executable.attributes {
                let ty = executable.vertex.symbols[name].ty;
                env.insert(name.clone(), self.attribute_value(location, index, ty));
            }
            let env = executable.vertex.run(env);
            let value = env["gl_Position"];
            let position = std::array::from_fn(|i| value.v[i] as f64);
            let mut varyings = Vec::new();
            for (name, ty) in &executable.varyings {
                varyings.extend(env[name].v[..ty.n()].iter().map(|v| *v as f64));
            }
            vertices.push(Vertex { position, varyings });
        }
        let mut work = indices
            .len()
            .checked_mul(executable.vertex.cost.max(1))
            .ok_or(Error::OutOfMemory)?;
        let rect = self.clip_rect(true);
        if rect[0] >= rect[2] || rect[1] >= rect[3] {
            return Ok(());
        }
        let mut triangles = Vec::new();
        let count = if mode == gl::TRIANGLES {
            vertices.len() / 3
        } else {
            vertices.len() - 2
        };
        for i in 0..count {
            let ix = match mode {
                gl::TRIANGLES => [i * 3, i * 3 + 1, i * 3 + 2],
                gl::TRIANGLE_FAN => [0, i + 1, i + 2],
                _ => {
                    if i % 2 == 0 {
                        [i, i + 1, i + 2]
                    } else {
                        [i + 1, i, i + 2]
                    }
                }
            };
            let input = ix.map(|i| vertices[i].clone());
            if input
                .iter()
                .any(|v| v.position.iter().any(|v| !v.is_finite()))
            {
                continue;
            }
            let polygon = clip(input);
            for i in 1..polygon.len().saturating_sub(1) {
                let p = [&polygon[0], &polygon[i], &polygon[i + 1]];
                if p.iter().any(|v| v.position[3] <= 0.) {
                    continue;
                }
                let mut v = p.map(|v| {
                    let iw = 1. / v.position[3];
                    Screen {
                        position: [
                            self.viewport[0] as f64
                                + (v.position[0] * iw + 1.) * 0.5 * self.viewport[2] as f64,
                            self.viewport[1] as f64
                                + (v.position[1] * iw + 1.) * 0.5 * self.viewport[3] as f64,
                            v.position[2] * iw,
                        ],
                        inverse_w: iw,
                        varyings: v.varyings.clone(),
                    }
                });
                let mut area = edge(&v[0], &v[1], v[2].position[0], v[2].position[1]);
                if area == 0. || !area.is_finite() {
                    continue;
                }
                if area < 0. {
                    v.swap(1, 2);
                    area = -area;
                }
                let minx = v
                    .iter()
                    .map(|v| v.position[0])
                    .fold(f64::INFINITY, f64::min)
                    .floor() as i64;
                let miny = v
                    .iter()
                    .map(|v| v.position[1])
                    .fold(f64::INFINITY, f64::min)
                    .floor() as i64;
                let maxx = v
                    .iter()
                    .map(|v| v.position[0])
                    .fold(f64::NEG_INFINITY, f64::max)
                    .ceil() as i64;
                let maxy = v
                    .iter()
                    .map(|v| v.position[1])
                    .fold(f64::NEG_INFINITY, f64::max)
                    .ceil() as i64;
                let bounds = [
                    minx.max(rect[0]),
                    miny.max(rect[1]),
                    maxx.min(rect[2]),
                    maxy.min(rect[3]),
                ];
                if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
                    continue;
                }
                let samples = (bounds[2] - bounds[0]) as usize * (bounds[3] - bounds[1]) as usize;
                work = work
                    .checked_add(
                        samples.saturating_mul(executable.fragment.cost + v[0].varyings.len() + 1),
                    )
                    .ok_or(Error::OutOfMemory)?;
                if work > MAX_WORK {
                    return Err(Error::OutOfMemory);
                }
                triangles.push(Triangle {
                    vertices: v,
                    bounds,
                    area,
                });
            }
        }
        // All array ranges, indices and resource budgets were checked
        // before the first pixel changes. Error draws leave the buffer intact.
        for triangle in triangles {
            self.rasterize(&triangle, &executable, &uniforms);
        }
        Ok(())
    }
    fn rasterize(&mut self, t: &Triangle, e: &Executable, uniforms: &BTreeMap<String, Value>) {
        let v = &t.vertices;
        let inc = [
            inclusive(&v[1], &v[2]),
            inclusive(&v[2], &v[0]),
            inclusive(&v[0], &v[1]),
        ];
        for y in t.bounds[1]..t.bounds[3] {
            for x in t.bounds[0]..t.bounds[2] {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let edges = [
                    edge(&v[1], &v[2], px, py),
                    edge(&v[2], &v[0], px, py),
                    edge(&v[0], &v[1], px, py),
                ];
                if edges
                    .iter()
                    .zip(inc)
                    .any(|(&a, inclusive)| a < 0. || (a == 0. && !inclusive))
                {
                    continue;
                }
                let bary = edges.map(|a| a / t.area);
                let depth = (bary
                    .iter()
                    .zip(v)
                    .map(|(b, v)| b * v.position[2])
                    .sum::<f64>()
                    * 0.5
                    + 0.5)
                    .clamp(0., 1.) as f32;
                let at = ((self.height as i64 - 1 - y) * self.width as i64 + x) as usize;
                if self.depth_test {
                    let old = self.depth[at];
                    let pass = match self.depth_func {
                        gl::NEVER => false,
                        gl::LESS => depth < old,
                        gl::EQUAL => depth == old,
                        gl::LEQUAL => depth <= old,
                        gl::GREATER => depth > old,
                        gl::NOTEQUAL => depth != old,
                        gl::GEQUAL => depth >= old,
                        _ => true,
                    };
                    if !pass {
                        continue;
                    }
                }
                let iw = bary
                    .iter()
                    .zip(v)
                    .map(|(b, v)| b * v.inverse_w)
                    .sum::<f64>();
                if iw <= 0. || !iw.is_finite() {
                    continue;
                }
                let mut env = uniforms.clone();
                let mut offset = 0;
                for (name, ty) in &e.varyings {
                    let mut value = Value::zero(*ty);
                    for j in 0..ty.n() {
                        value.v[j] = (bary
                            .iter()
                            .zip(v)
                            .map(|(b, v)| b * v.varyings[offset + j] * v.inverse_w)
                            .sum::<f64>()
                            / iw) as f32;
                    }
                    offset += ty.n();
                    env.insert(name.clone(), value);
                }
                env.insert(
                    "gl_FragCoord".into(),
                    Value::from(Ty::Vec4, &[px as f32, py as f32, depth, iw as f32]),
                );
                let out = e
                    .fragment
                    .run_with_sampler(env, &|unit, uv| self.sample_texture(unit, uv));
                let color = out["gl_FragColor"];
                for k in 0..4 {
                    if self.color_mask[k] {
                        self.rgba[at * 4 + k] = byte(color.v[k]);
                    }
                }
                if self.depth_test && self.depth_mask {
                    self.depth[at] = depth;
                }
            }
        }
    }
}
