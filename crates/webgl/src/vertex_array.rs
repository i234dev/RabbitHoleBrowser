//! WebGL2 vertex array ownership. Generic attribute constants and ARRAY_BUFFER
//! remain context-global; captured attribute pointers and the element buffer do not.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct VertexArray {
    attributes: [Attribute; MAX_VERTEX_ATTRIBS as usize],
    element: u32,
    bound: bool,
}
impl Default for VertexArray {
    fn default() -> Self {
        Self { attributes: [Attribute::default(); MAX_VERTEX_ATTRIBS as usize], element: 0, bound: false }
    }
}
impl Context {
    pub fn new_webgl2(width: u32, height: u32) -> Result<Self> {
        let mut context = Self::new_with_glsl_version(width, height, 300)?;
        context.context_version = 2;
        Ok(context)
    }
    pub fn context_version(&self) -> u32 { self.context_version }
    fn require_webgl2(&self) -> Result<()> {
        if self.context_version == 2 { Ok(()) } else { Err(Error::InvalidOperation) }
    }
    pub fn create_vertex_array(&mut self) -> Result<u32> {
        self.require_webgl2()?;
        if self.vertex_arrays.len() >= 4097 { return Err(Error::OutOfMemory); }
        let id = self.id();
        if id == 0 { return Err(Error::OutOfMemory); }
        self.vertex_arrays.insert(id, VertexArray::default());
        Ok(id)
    }
    pub fn bind_vertex_array(&mut self, id: u32) -> Result<()> {
        self.require_webgl2()?;
        let mut next = *self.vertex_arrays.get(&id).ok_or(Error::InvalidOperation)?;
        self.vertex_arrays.insert(self.current_vertex_array, VertexArray {
            attributes: self.attributes, element: self.element_buffer, bound: true,
        });
        if id == self.current_vertex_array { return Ok(()); }
        next.bound = true;
        self.vertex_arrays.insert(id, next);
        self.attributes = next.attributes;
        self.element_buffer = next.element;
        self.current_vertex_array = id;
        self.collect_deleted_buffers();
        Ok(())
    }
    pub fn vertex_array_binding(&self) -> u32 { self.current_vertex_array }
    pub fn is_vertex_array(&self, id: u32) -> bool {
        self.context_version == 2 && id != 0 && self.vertex_arrays.get(&id).is_some_and(|v| v.bound)
    }
    pub fn delete_vertex_array(&mut self, id: u32) -> Result<()> {
        self.require_webgl2()?;
        if id == 0 { return Ok(()); }
        if !self.vertex_arrays.contains_key(&id) { return Err(Error::InvalidOperation); }
        if self.current_vertex_array == id { self.bind_vertex_array(0)?; }
        self.vertex_arrays.remove(&id);
        self.collect_deleted_buffers();
        Ok(())
    }
    pub(super) fn collect_deleted_buffers(&mut self) {
        let current = self.current_vertex_array;
        let referenced = |id: u32| self.array_buffer == id || self.element_buffer == id ||
            self.attributes.iter().any(|a| a.buffer == id) || self.vertex_arrays.iter().any(|(&vao, v)|
                vao != current && (v.element == id || v.attributes.iter().any(|a| a.buffer == id)));
        let dead: Vec<_> = self.buffers.iter().filter_map(|(&id, b)| (b.deleted && !referenced(id)).then_some(id)).collect();
        for id in dead { self.buffer_bytes -= self.buffers.remove(&id).unwrap().bytes.len(); }
    }
}
