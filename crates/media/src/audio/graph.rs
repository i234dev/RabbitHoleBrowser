//! Private, device-independent context graph. No JS, threads or audio sinks.
//! Canonical stereo blocks, sample-clock schedules and global 128-frame k-rate
//! boundaries are shared by all nodes in this one renderer.

use super::compressor::{Compressor, Frame, Parameters, ProcessError, QUANTUM};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_NODES: usize = 256;
const MAX_EDGES: usize = 2048;
const MAX_EVENTS: usize = 1024;
const MAX_PAYLOAD: usize = 64 * 1024 * 1024;
static NEXT_CONTEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GraphError {
    SampleRate,
    Capacity,
    Context,
    StaleNode,
    Port,
    Cycle,
    Boundary,
    EventOrder,
    Value,
    Kind,
    BlockSize,
    ClockOverflow,
    Faulted,
    Internal,
    Dsp(ProcessError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NodeId {
    context: u64,
    slot: usize,
    generation: u32,
}

#[derive(Clone, Debug)]
pub(super) struct Timeline<T> {
    initial: T,
    events: Vec<(u64, T)>,
}

impl<T: Copy> Timeline<T> {
    pub fn new(initial: T, events: Vec<(u64, T)>) -> Result<Self, GraphError> {
        if events.len() > MAX_EVENTS {
            return Err(GraphError::Capacity);
        }
        if events.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(GraphError::EventOrder);
        }
        Ok(Self { initial, events })
    }

    pub fn constant(initial: T) -> Self {
        Self { initial, events: Vec::new() }
    }

    fn at(&self, frame: u64) -> T {
        let n = self.events.partition_point(|(at, _)| *at <= frame);
        if n == 0 { self.initial } else { self.events[n - 1].1 }
    }

    fn validate(&self, check: impl Fn(T) -> Result<(), GraphError>) -> Result<(), GraphError> {
        check(self.initial)?;
        for (_, value) in &self.events {
            check(*value)?;
        }
        Ok(())
    }

    fn payload(&self) -> usize {
        self.events.len() * std::mem::size_of::<(u64, T)>()
    }
}

#[derive(Clone, Debug)]
pub(super) struct SourceBuffer {
    frames: Arc<[Frame]>,
}

impl SourceBuffer {
    pub fn mono(samples: &[f32]) -> Result<Self, GraphError> {
        Self::build(samples.len(), samples.iter().map(|&sample| [sample, sample]))
    }

    pub fn stereo(samples: &[Frame]) -> Result<Self, GraphError> {
        Self::build(samples.len(), samples.iter().copied())
    }

    fn build(len: usize, samples: impl Iterator<Item = Frame>) -> Result<Self, GraphError> {
        let bytes = len.checked_mul(std::mem::size_of::<Frame>()).ok_or(GraphError::Capacity)?;
        if len == 0 || bytes > MAX_PAYLOAD {
            return Err(GraphError::Capacity);
        }
        // Conversion is control-side and owned; no borrowed typed-array data.
        let frames: Vec<_> = samples.collect();
        if frames.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(GraphError::Value);
        }
        Ok(Self { frames: frames.into() })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Schedule {
    pub start: u64,
    pub stop: Option<u64>,
}

impl Schedule {
    fn active(self, frame: u64) -> bool {
        frame >= self.start && self.stop.is_none_or(|stop| frame < stop)
    }
}

enum Node {
    Destination,
    Buffer { buffer: SourceBuffer, schedule: Schedule },
    Sine { frequency: f32, schedule: Schedule },
    Gain(Timeline<f32>),
    Compressor {
        timeline: Timeline<Parameters>,
        state: Compressor,
        snapshot: Parameters,
        quantum: Option<u64>,
    },
}

impl Node {
    fn has_input(&self) -> bool {
        matches!(self, Self::Destination | Self::Gain(_) | Self::Compressor { .. })
    }

    fn has_output(&self) -> bool {
        !matches!(self, Self::Destination)
    }
}

struct Slot {
    generation: u32,
    node: Option<Node>,
    block: [Frame; QUANTUM],
    payload: usize,
}

struct PlanEntry {
    slot: usize,
    inputs: Vec<usize>,
}

/// Graph mutation is control-side and allowed only at global boundaries.
/// A later adapter may queue such changes; this core does not accept/ignore
/// mid-quantum changes or start any device worker.
pub(super) struct ContextRenderer {
    context: u64,
    sample_rate: u32,
    cursor: u64,
    slots: Vec<Slot>,
    edges: Vec<(NodeId, NodeId)>,
    plan: Vec<PlanEntry>,
    payload: usize,
    faulted: bool,
}

impl ContextRenderer {
    pub fn new(sample_rate: u32) -> Result<Self, GraphError> {
        if !(3000..=768000).contains(&sample_rate) {
            return Err(GraphError::SampleRate);
        }
        let context = NEXT_CONTEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| GraphError::Capacity)?;
        Ok(Self {
            context, sample_rate, cursor: 0,
            slots: vec![Slot { generation: 1, node: Some(Node::Destination), block: [[0.0; 2]; QUANTUM], payload: std::mem::size_of::<Slot>() }],
            edges: Vec::new(), plan: vec![PlanEntry { slot: 0, inputs: Vec::new() }],
            payload: std::mem::size_of::<Slot>(), faulted: false,
        })
    }

    pub fn destination(&self) -> NodeId {
        self.id(0)
    }

    pub fn current_frame(&self) -> u64 {
        self.cursor
    }

    fn id(&self, slot: usize) -> NodeId {
        NodeId { context: self.context, slot, generation: self.slots[slot].generation }
    }

    fn lookup(&self, id: NodeId) -> Result<usize, GraphError> {
        if id.context != self.context {
            return Err(GraphError::Context);
        }
        match self.slots.get(id.slot) {
            Some(slot) if slot.generation == id.generation && slot.node.is_some() => Ok(id.slot),
            _ => Err(GraphError::StaleNode),
        }
    }

    fn boundary(&self) -> Result<(), GraphError> {
        if self.faulted { return Err(GraphError::Faulted); }
        if self.cursor % QUANTUM as u64 != 0 { return Err(GraphError::Boundary); }
        Ok(())
    }

    fn add(&mut self, node: Node, extra: usize) -> Result<NodeId, GraphError> {
        self.boundary()?;
        let cost = extra.checked_add(std::mem::size_of::<Slot>()).ok_or(GraphError::Capacity)?;
        if self.payload.checked_add(cost).is_none_or(|bytes| bytes > MAX_PAYLOAD) {
            return Err(GraphError::Capacity);
        }
        let vacant = self.slots.iter().position(|slot| slot.node.is_none() && slot.generation < u32::MAX);
        let index = if let Some(index) = vacant {
            self.slots[index].node = Some(node);
            self.slots[index].payload = cost;
            index
        } else {
            if self.slots.len() == MAX_NODES { return Err(GraphError::Capacity); }
            let index = self.slots.len();
            self.slots.push(Slot { generation: 1, node: Some(node), block: [[0.0; 2]; QUANTUM], payload: cost });
            index
        };
        self.payload += cost;
        Ok(self.id(index))
    }

    pub fn add_buffer(&mut self, buffer: SourceBuffer, schedule: Schedule) -> Result<NodeId, GraphError> {
        let payload = buffer.frames.len() * std::mem::size_of::<Frame>();
        self.add(Node::Buffer { buffer, schedule }, payload)
    }

    pub fn add_sine(&mut self, frequency: f32, schedule: Schedule) -> Result<NodeId, GraphError> {
        if !frequency.is_finite() || frequency.abs() > self.sample_rate as f32 * 0.5 {
            return Err(GraphError::Value);
        }
        self.add(Node::Sine { frequency, schedule }, 0)
    }

    pub fn add_gain(&mut self, timeline: Timeline<f32>) -> Result<NodeId, GraphError> {
        timeline.validate(finite_gain)?;
        let payload = timeline.payload();
        self.add(Node::Gain(timeline), payload)
    }

    pub fn add_compressor(&mut self, timeline: Timeline<Parameters>) -> Result<NodeId, GraphError> {
        timeline.validate(valid_parameters)?;
        let delay_frames = (f64::from(self.sample_rate) * 0.006).floor() as usize + 2;
        let payload = timeline.payload() + delay_frames * std::mem::size_of::<Frame>();
        // Check cost before creating this node's delay allocation.
        self.boundary()?;
        if self.payload.checked_add(payload + std::mem::size_of::<Slot>()).is_none_or(|bytes| bytes > MAX_PAYLOAD) {
            return Err(GraphError::Capacity);
        }
        let state = Compressor::new(self.sample_rate).map_err(GraphError::Dsp)?;
        let snapshot = timeline.initial;
        self.add(Node::Compressor { timeline, state, snapshot, quantum: None }, payload)
    }

    pub fn connect(&mut self, from: NodeId, to: NodeId, output: u32, input: u32) -> Result<(), GraphError> {
        self.boundary()?;
        let source = self.lookup(from)?;
        let destination = self.lookup(to)?;
        if output != 0 || input != 0
            || !self.slots[source].node.as_ref().is_some_and(Node::has_output)
            || !self.slots[destination].node.as_ref().is_some_and(Node::has_input) {
            return Err(GraphError::Port);
        }
        if self.edges.contains(&(from, to)) { return Ok(()); }
        if self.edges.len() == MAX_EDGES { return Err(GraphError::Capacity); }
        let mut edges = self.edges.clone();
        edges.push((from, to));
        let plan = self.prepare(&edges, None)?;
        self.edges = edges;
        self.plan = plan;
        Ok(())
    }

    pub fn disconnect(&mut self, from: NodeId, to: NodeId) -> Result<(), GraphError> {
        self.boundary()?;
        self.lookup(from)?;
        self.lookup(to)?;
        let edges: Vec<_> = self.edges.iter().copied().filter(|edge| *edge != (from, to)).collect();
        let plan = self.prepare(&edges, None)?;
        self.edges = edges;
        self.plan = plan;
        Ok(())
    }

    pub fn remove(&mut self, id: NodeId) -> Result<(), GraphError> {
        self.boundary()?;
        let slot = self.lookup(id)?;
        if slot == 0 { return Err(GraphError::Kind); }
        let edges: Vec<_> = self.edges.iter().copied().filter(|(from, to)| *from != id && *to != id).collect();
        let plan = self.prepare(&edges, Some(slot))?;
        self.payload -= self.slots[slot].payload;
        self.slots[slot].node = None;
        self.slots[slot].payload = 0;
        self.slots[slot].generation = self.slots[slot].generation.saturating_add(1);
        self.edges = edges;
        self.plan = plan;
        Ok(())
    }

    fn prepare(&self, edges: &[(NodeId, NodeId)], excluded: Option<usize>) -> Result<Vec<PlanEntry>, GraphError> {
        let n = self.slots.len();
        let mut inputs = vec![Vec::new(); n];
        let mut outputs = vec![Vec::new(); n];
        let mut degree = vec![0usize; n];
        let live = |index: usize| Some(index) != excluded && self.slots[index].node.is_some();
        for &(from, to) in edges {
            let from = self.lookup(from)?;
            let to = self.lookup(to)?;
            if !live(from) || !live(to) { return Err(GraphError::Internal); }
            inputs[to].push(from);
            outputs[from].push(to);
            degree[to] += 1;
        }
        let count = (0..n).filter(|&i| live(i)).count();
        let mut ready: VecDeque<_> = (0..n).filter(|&i| live(i) && degree[i] == 0).collect();
        let mut order = Vec::with_capacity(count);
        while let Some(index) = ready.pop_front() {
            order.push(index);
            for &next in &outputs[index] {
                degree[next] -= 1;
                if degree[next] == 0 { ready.push_back(next); }
            }
        }
        if order.len() != count { return Err(GraphError::Cycle); }
        let mut used = vec![false; n];
        let mut pending = vec![0usize];
        while let Some(index) = pending.pop() {
            if used[index] { continue; }
            used[index] = true;
            pending.extend(inputs[index].iter().copied());
        }
        Ok(order.into_iter().filter(|&i| used[i]).map(|slot| PlanEntry { slot, inputs: std::mem::take(&mut inputs[slot]) }).collect())
    }

    /// Caller-owned output is committed only after all segments succeed. An
    /// impossible internal render failure faults this context; it cannot retry
    /// partially advanced DSP state or publish half of a requested block.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), GraphError> {
        if self.faulted { return Err(GraphError::Faulted); }
        if output.is_empty() || output.len() > QUANTUM { return Err(GraphError::BlockSize); }
        self.cursor.checked_add(output.len() as u64).ok_or(GraphError::ClockOverflow)?;
        let mut staged = [[0.0; 2]; QUANTUM];
        let mut done = 0;
        while done < output.len() {
            let remaining = QUANTUM - (self.cursor % QUANTUM as u64) as usize;
            let frames = remaining.min(output.len() - done);
            if let Err(error) = self.segment(frames) {
                self.faulted = true;
                return Err(error);
            }
            staged[done..done + frames].copy_from_slice(&self.slots[0].block[..frames]);
            self.cursor += frames as u64;
            done += frames;
        }
        output.copy_from_slice(&staged[..output.len()]);
        Ok(())
    }

    fn segment(&mut self, frames: usize) -> Result<(), GraphError> {
        let quantum = self.cursor - self.cursor % QUANTUM as u64;
        for entry in &self.plan {
            let mut mixed = [[0.0; 2]; QUANTUM];
            for (i, frame) in mixed[..frames].iter_mut().enumerate() {
                for (channel, out) in frame.iter_mut().enumerate() {
                    let sum: f64 = entry.inputs.iter().map(|&source| f64::from(self.slots[source].block[i][channel])).sum();
                    *out = pcm(sum);
                }
            }
            let slot = &mut self.slots[entry.slot];
            let node = slot.node.as_mut().ok_or(GraphError::Internal)?;
            match node {
                Node::Destination => slot.block[..frames].copy_from_slice(&mixed[..frames]),
                Node::Buffer { buffer, schedule } => {
                    for (i, frame) in slot.block[..frames].iter_mut().enumerate() {
                        let clock = self.cursor + i as u64;
                        *frame = if schedule.active(clock) {
                            usize::try_from(clock - schedule.start).ok().and_then(|i| buffer.frames.get(i)).copied().unwrap_or([0.0; 2])
                        } else { [0.0; 2] };
                    }
                }
                Node::Sine { frequency, schedule } => {
                    for (i, frame) in slot.block[..frames].iter_mut().enumerate() {
                        let clock = self.cursor + i as u64;
                        let sample = if schedule.active(clock) {
                            ((clock - schedule.start) as f64 * f64::from(*frequency) * std::f64::consts::TAU / f64::from(self.sample_rate)).sin() as f32
                        } else { 0.0 };
                        *frame = [sample; 2];
                    }
                }
                Node::Gain(timeline) => {
                    for i in 0..frames {
                        let gain = f64::from(timeline.at(self.cursor + i as u64));
                        slot.block[i] = [pcm(f64::from(mixed[i][0]) * gain), pcm(f64::from(mixed[i][1]) * gain)];
                    }
                }
                Node::Compressor { timeline, state, snapshot, quantum: previous } => {
                    if *previous != Some(quantum) {
                        *snapshot = timeline.at(quantum);
                        *previous = Some(quantum);
                    }
                    state.process(&mixed[..frames], &mut slot.block[..frames], *snapshot).map_err(GraphError::Dsp)?;
                }
            }
        }
        Ok(())
    }
}

fn finite_gain(value: f32) -> Result<(), GraphError> {
    if value.is_finite() { Ok(()) } else { Err(GraphError::Value) }
}

fn valid_parameters(value: Parameters) -> Result<(), GraphError> {
    value.validate().map_err(GraphError::Dsp)
}

fn pcm(value: f64) -> f32 {
    value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: Schedule = Schedule { start: 0, stop: None };

    fn hard() -> Parameters {
        Parameters { threshold: -24.0, knee: 0.0, ratio: 12.0, attack: 0.0, release: 0.0 }
    }

    fn render(graph: &mut ContextRenderer, frames: usize, chunks: &[usize]) -> Vec<Frame> {
        assert!(!chunks.is_empty() && chunks.iter().all(|&n| (1..=QUANTUM).contains(&n)));
        let mut result = vec![[0.0; 2]; frames];
        let mut done = 0;
        let mut chunk = 0;
        while done < frames {
            let count = chunks[chunk % chunks.len()].min(frames - done);
            graph.render(&mut result[done..done + count]).unwrap();
            done += count;
            chunk += 1;
        }
        result
    }

    fn compressed(frames: &[Frame]) -> (ContextRenderer, NodeId) {
        let mut graph = ContextRenderer::new(48000).unwrap();
        let source = graph.add_buffer(SourceBuffer::stereo(frames).unwrap(), START).unwrap();
        let effect = graph.add_compressor(Timeline::constant(hard())).unwrap();
        graph.connect(source, effect, 0, 0).unwrap();
        graph.connect(effect, graph.destination(), 0, 0).unwrap();
        (graph, effect)
    }

    #[test]
    fn mono_is_upmixed_before_mono_and_stereo_sources_are_summed() {
        let mut graph = ContextRenderer::new(48000).unwrap();
        let mono = graph.add_buffer(SourceBuffer::mono(&[0.25; 256]).unwrap(), START).unwrap();
        graph.connect(mono, graph.destination(), 0, 0).unwrap();
        assert!(render(&mut graph, 128, &[67, 61]).iter().all(|frame| *frame == [0.25; 2]));
        let stereo = graph.add_buffer(SourceBuffer::stereo(&[[0.125, -0.5]; 256]).unwrap(), START).unwrap();
        graph.connect(stereo, graph.destination(), 0, 0).unwrap();
        assert!(render(&mut graph, 128, &[128]).iter().all(|frame| *frame == [0.375, -0.25]));
    }

    #[test]
    fn two_sources_compress_as_their_sum_and_not_as_two_compressors() {
        let mut graph = ContextRenderer::new(48000).unwrap();
        let a = graph.add_buffer(SourceBuffer::mono(&vec![0.4; 4096]).unwrap(), START).unwrap();
        let b = graph.add_buffer(SourceBuffer::stereo(&vec![[0.4, -0.2]; 4096]).unwrap(), START).unwrap();
        let effect = graph.add_compressor(Timeline::constant(hard())).unwrap();
        graph.connect(a, effect, 0, 0).unwrap();
        graph.connect(b, effect, 0, 0).unwrap();
        graph.connect(effect, graph.destination(), 0, 0).unwrap();
        let output = render(&mut graph, 4096, &[67, 64, 7, 128]);
        let (mut equivalent, _) = compressed(&vec![[0.8, 0.2]; 4096]);
        assert_eq!(output, render(&mut equivalent, 4096, &[128]));
        let (mut wrong_a, _) = compressed(&vec![[0.4; 2]; 4096]);
        let (mut wrong_b, _) = compressed(&vec![[0.4, -0.2]; 4096]);
        let wrong_a = render(&mut wrong_a, 4096, &[128]);
        let wrong_b = render(&mut wrong_b, 4096, &[128]);
        assert!((output[4095][0] - wrong_a[4095][0] - wrong_b[4095][0]).abs() > 0.1);
        assert!((output[4095][1] / output[4095][0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn stateful_fanout_is_evaluated_once_and_is_independent_of_node_insertion_order() {
        let input = vec![[0.8, 0.2]; 4096];
        let (mut direct, _) = compressed(&input);
        let reference = render(&mut direct, 4096, &[128]);
        for gains_first in [false, true] {
            let mut graph = ContextRenderer::new(48000).unwrap();
            let (effect, a, b) = if gains_first {
                let a = graph.add_gain(Timeline::constant(0.5)).unwrap();
                let b = graph.add_gain(Timeline::constant(0.5)).unwrap();
                (graph.add_compressor(Timeline::constant(hard())).unwrap(), a, b)
            } else {
                let effect = graph.add_compressor(Timeline::constant(hard())).unwrap();
                (effect, graph.add_gain(Timeline::constant(0.5)).unwrap(), graph.add_gain(Timeline::constant(0.5)).unwrap())
            };
            let source = graph.add_buffer(SourceBuffer::stereo(&input).unwrap(), START).unwrap();
            graph.connect(source, effect, 0, 0).unwrap();
            for gain in [a, b] {
                graph.connect(effect, gain, 0, 0).unwrap();
                graph.connect(gain, graph.destination(), 0, 0).unwrap();
            }
            assert_eq!(reference, render(&mut graph, 4096, &[7, 127, 67]));
        }
    }

    #[test]
    fn gain_before_compression_differs_from_gain_after_compression() {
        let mut last = Vec::new();
        for before in [true, false] {
            let mut graph = ContextRenderer::new(48000).unwrap();
            let source = graph.add_buffer(SourceBuffer::mono(&vec![1.0; 4096]).unwrap(), START).unwrap();
            let gain = graph.add_gain(Timeline::constant(0.5)).unwrap();
            let effect = graph.add_compressor(Timeline::constant(hard())).unwrap();
            let path = if before { [source, gain, effect, graph.destination()] } else { [source, effect, gain, graph.destination()] };
            for edge in path.windows(2) { graph.connect(edge[0], edge[1], 0, 0).unwrap(); }
            last.push(render(&mut graph, 4096, &[128])[4095][0]);
        }
        assert!((last[0] - last[1]).abs() > 0.1);
    }

    #[test]
    fn a_rate_events_and_global_k_rate_boundaries_survive_partial_render_calls() {
        let initial = Parameters { ratio: 1.0, ..hard() };
        let mut graph = ContextRenderer::new(8000).unwrap();
        let source = graph.add_buffer(SourceBuffer::mono(&[1.0; 1024]).unwrap(), START).unwrap();
        let effect = graph.add_compressor(Timeline::new(initial, vec![(70, hard())]).unwrap()).unwrap();
        let gain = graph.add_gain(Timeline::new(1.0, vec![(70, 0.5)]).unwrap()).unwrap();
        for (a, b) in [(source, effect), (effect, gain), (gain, graph.destination())] {
            graph.connect(a, b, 0, 0).unwrap();
        }
        let output = render(&mut graph, 1024, &[67, 64, 7, 128]);
        let mut reference = Compressor::new(8000).unwrap();
        let mut expected = vec![[0.0; 2]; 1024];
        for (i, block) in expected.chunks_mut(128).enumerate() {
            reference.process(&[[1.0; 2]; 128], block, if i == 0 { initial } else { hard() }).unwrap();
        }
        for (i, frame) in expected.iter_mut().enumerate() {
            if i >= 70 { *frame = [frame[0] * 0.5, frame[1] * 0.5]; }
        }
        assert_eq!(output, expected);
        assert_eq!(output[69], [1.0; 2]);
        assert_eq!(output[70], [0.5; 2]);
        assert_eq!(output[127], [0.5; 2]);
        assert_ne!(output[128], [0.5; 2]);
        assert_eq!(graph.current_frame(), 1024);
    }

    #[test]
    fn scheduled_sources_stop_at_the_exact_frame_and_compressor_tail_survives() {
        let mut graph = ContextRenderer::new(8000).unwrap();
        let source = graph.add_buffer(SourceBuffer::mono(&[0.25, 0.5, 0.75, 1.0]).unwrap(), Schedule { start: 10, stop: Some(13) }).unwrap();
        let effect = graph.add_compressor(Timeline::constant(Parameters { ratio: 1.0, ..hard() })).unwrap();
        graph.connect(source, effect, 0, 0).unwrap();
        graph.connect(effect, graph.destination(), 0, 0).unwrap();
        let output = render(&mut graph, 70, &[7, 11]);
        assert!(output[..58].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(&output[58..61], &[[0.25; 2], [0.5; 2], [0.75; 2]]);
        assert!(output[61..].iter().all(|frame| *frame == [0.0; 2]));

        let mut sine = ContextRenderer::new(8000).unwrap();
        let source = sine.add_sine(1000.0, Schedule { start: 7, stop: Some(17) }).unwrap();
        sine.connect(source, sine.destination(), 0, 0).unwrap();
        let output = render(&mut sine, 31, &[1, 7, 11]);
        assert!(output[..7].iter().chain(output[17..].iter()).all(|frame| *frame == [0.0; 2]));
        assert!((output[9][0] - 1.0).abs() < 1e-6);
        assert!((output[13][0] + 1.0).abs() < 1e-6);
        assert!(output.iter().all(|frame| frame[0] == frame[1]));
    }

    #[test]
    fn bad_graph_changes_leave_the_prepared_graph_and_audio_unchanged() {
        let input = vec![[0.8; 2]; 512];
        let (mut graph, effect) = compressed(&input);
        let before_edges = graph.edges.clone();
        let before_payload = graph.payload;
        let a = graph.add_gain(Timeline::constant(1.0)).unwrap();
        let b = graph.add_gain(Timeline::constant(1.0)).unwrap();
        graph.connect(a, b, 0, 0).unwrap();
        let accepted = graph.edges.clone();
        assert_eq!(graph.connect(b, a, 0, 0), Err(GraphError::Cycle));
        assert_eq!(graph.connect(effect, effect, 0, 0), Err(GraphError::Cycle));
        assert_eq!(graph.connect(graph.destination(), effect, 0, 0), Err(GraphError::Port));
        assert_eq!(graph.connect(a, effect, 1, 0), Err(GraphError::Port));
        let foreign = ContextRenderer::new(48000).unwrap();
        assert_eq!(graph.connect(foreign.destination(), effect, 0, 0), Err(GraphError::Context));
        assert_eq!(graph.edges, accepted);
        assert_eq!(graph.add_gain(Timeline::constant(f32::NAN)), Err(GraphError::Value));
        assert_eq!(graph.add_gain(Timeline::new(1.0, vec![(128, f32::INFINITY)]).unwrap()), Err(GraphError::Value));
        assert_eq!(graph.add_compressor(Timeline::constant(Parameters { ratio: 0.0, ..hard() })), Err(GraphError::Dsp(ProcessError::Parameters)));
        assert_eq!(graph.add_compressor(Timeline::new(hard(), vec![(128, Parameters { ratio: f32::NAN, ..hard() })]).unwrap()), Err(GraphError::Dsp(ProcessError::Parameters)));
        assert_eq!(graph.add_sine(f32::INFINITY, START), Err(GraphError::Value));
        assert!(SourceBuffer::mono(&[0.0, f32::INFINITY]).is_err());
        graph.remove(a).unwrap();
        graph.remove(b).unwrap();
        assert_eq!(graph.edges, before_edges);
        assert_eq!(graph.payload, before_payload);
        let (mut control, _) = compressed(&input);
        assert_eq!(render(&mut graph, 512, &[67, 64]), render(&mut control, 512, &[128]));
    }

    #[test]
    fn generation_and_boundary_guards_reject_stale_or_mid_quantum_mutations() {
        let mut graph = ContextRenderer::new(48000).unwrap();
        let old = graph.add_gain(Timeline::constant(1.0)).unwrap();
        graph.remove(old).unwrap();
        let new = graph.add_gain(Timeline::constant(0.5)).unwrap();
        assert_eq!(old.slot, new.slot);
        assert_ne!(old.generation, new.generation);
        assert_eq!(graph.connect(old, graph.destination(), 0, 0), Err(GraphError::StaleNode));
        let source = graph.add_buffer(SourceBuffer::mono(&[1.0; 256]).unwrap(), START).unwrap();
        graph.connect(source, new, 0, 0).unwrap();
        graph.connect(new, graph.destination(), 0, 0).unwrap();
        assert!(render(&mut graph, 67, &[67]).iter().all(|frame| *frame == [0.5; 2]));
        assert_eq!(graph.remove(new), Err(GraphError::Boundary));
        assert_eq!(graph.disconnect(source, new), Err(GraphError::Boundary));
        assert_eq!(graph.add_gain(Timeline::constant(1.0)), Err(GraphError::Boundary));
        assert!(render(&mut graph, 61, &[61]).iter().all(|frame| *frame == [0.5; 2]));
        graph.remove(new).unwrap();
        assert!(render(&mut graph, 128, &[128]).iter().all(|frame| *frame == [0.0; 2]));
    }

    #[test]
    fn independent_contexts_budgets_clock_and_output_finiteness_have_explicit_guards() {
        assert!(matches!(ContextRenderer::new(0), Err(GraphError::SampleRate)));
        assert!(matches!(Timeline::new(1.0, vec![(3, 1.0), (3, 2.0)]), Err(GraphError::EventOrder)));
        assert!(matches!(Timeline::new(1.0, vec![(0, 1.0); MAX_EVENTS + 1]), Err(GraphError::Capacity)));
        assert!(matches!(SourceBuffer::build(MAX_PAYLOAD / std::mem::size_of::<Frame>() + 1, std::iter::empty()), Err(GraphError::Capacity)));
        let mut full = ContextRenderer::new(8000).unwrap();
        for _ in 1..MAX_NODES { full.add_gain(Timeline::constant(1.0)).unwrap(); }
        assert_eq!(full.add_gain(Timeline::constant(1.0)), Err(GraphError::Capacity));

        let mut loud = ContextRenderer::new(48000).unwrap();
        let source = loud.add_buffer(SourceBuffer::stereo(&[[f32::MAX, -f32::MAX]; 128]).unwrap(), START).unwrap();
        let gain = loud.add_gain(Timeline::constant(f32::MAX)).unwrap();
        loud.connect(source, gain, 0, 0).unwrap();
        loud.connect(gain, loud.destination(), 0, 0).unwrap();
        let mut independent = ContextRenderer::new(48000).unwrap();
        assert!(render(&mut loud, 128, &[128]).iter().all(|frame| *frame == [f32::MAX, -f32::MAX]));
        assert!(render(&mut independent, 128, &[128]).iter().all(|frame| *frame == [0.0; 2]));
        let mut out = [[17.0; 2]; 129];
        assert_eq!(independent.render(&mut out), Err(GraphError::BlockSize));
        assert_eq!(out, [[17.0; 2]; 129]);
        independent.cursor = u64::MAX;
        assert_eq!(independent.render(&mut out[..1]), Err(GraphError::ClockOverflow));
        assert_eq!(out, [[17.0; 2]; 129]);
    }

    #[test]
    fn an_internal_late_failure_faults_only_its_context_and_publishes_no_partial_block() {
        let (mut graph, effect) = compressed(&[[1.0; 2]; 512]);
        render(&mut graph, 64, &[64]);
        // Corruption cannot enter through add_compressor: its full timeline is
        // validated. Deliberately bypass it to exercise defensive fail-stop.
        if let Some(Node::Compressor { timeline, .. }) = graph.slots[effect.slot].node.as_mut() {
            timeline.events.push((128, Parameters { ratio: f32::NAN, ..hard() }));
        } else { panic!("compressor fixture"); }
        let mut output = [[17.0; 2]; 128];
        assert_eq!(graph.render(&mut output), Err(GraphError::Dsp(ProcessError::Parameters)));
        assert_eq!(output, [[17.0; 2]; 128]);
        assert_eq!(graph.current_frame(), 128);
        assert_eq!(graph.render(&mut output), Err(GraphError::Faulted));
        let (mut other, _) = compressed(&[[1.0; 2]; 512]);
        assert!(render(&mut other, 128, &[128]).iter().all(|frame| *frame == [0.0; 2]));
    }
}
