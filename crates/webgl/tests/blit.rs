use rabbithole_webgl::{gl::*, Context, Error};

const QUAD: [u8; 16] = [255,0,0,255, 0,255,0,255, 0,0,255,255, 255,255,255,255];
fn fbo(c: &mut Context, w: u32, h: u32, pixels: Option<&[u8]>, depth: bool) -> (u32,u32,u32) {
    let t=c.create_texture(); c.bind_texture(TEXTURE_2D,t).unwrap();
    c.tex_image_2d(TEXTURE_2D,0,RGBA,w,h,0,RGBA,UNSIGNED_BYTE,pixels).unwrap();
    let f=c.create_framebuffer(); c.bind_framebuffer(FRAMEBUFFER,f).unwrap();
    c.framebuffer_texture_2d(FRAMEBUFFER,COLOR_ATTACHMENT0,TEXTURE_2D,t,0).unwrap();
    let r=if depth {
        let r=c.create_renderbuffer(); c.bind_renderbuffer(RENDERBUFFER,r).unwrap();
        c.renderbuffer_storage(RENDERBUFFER,DEPTH_COMPONENT16,w,h).unwrap();
        c.framebuffer_renderbuffer(FRAMEBUFFER,DEPTH_ATTACHMENT,RENDERBUFFER,r).unwrap(); r
    } else {0};
    (f,t,r)
}
fn bind(c:&mut Context, read:u32, draw:u32) {
    c.bind_framebuffer(READ_FRAMEBUFFER,read).unwrap(); c.bind_framebuffer(DRAW_FRAMEBUFFER,draw).unwrap();
}
fn pixels(c:&mut Context,f:u32,w:u32,h:u32)->Vec<u8> {
    c.bind_framebuffer(READ_FRAMEBUFFER,f).unwrap(); c.read_pixels(0,0,w,h).unwrap()
}
fn pixel(c:&mut Context,f:u32,x:i32,y:i32)->Vec<u8> {
    c.bind_framebuffer(READ_FRAMEBUFFER,f).unwrap(); c.read_pixels(x,y,1,1).unwrap()
}

#[test]
fn color_blits_between_fbos_and_default_keep_orientation_and_bindings() {
    let mut c=Context::new_webgl2(2,2).unwrap();
    let (src,_,_)=fbo(&mut c,2,2,Some(&QUAD),false); let (dst,_,_)=fbo(&mut c,2,2,None,false);
    bind(&mut c,src,dst); c.blit_framebuffer([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(c.framebuffer_binding(READ_FRAMEBUFFER).unwrap(),src);
    assert_eq!(c.framebuffer_binding(DRAW_FRAMEBUFFER).unwrap(),dst);
    assert!(c.framebuffer_rgba().iter().all(|v|*v==0)); assert_eq!(pixels(&mut c,dst,2,2),QUAD);
    bind(&mut c,dst,0);c.blit_framebuffer([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixels(&mut c,0,2,2),QUAD);assert_eq!(&c.framebuffer_rgba()[..8],&QUAD[8..]);
    bind(&mut c,0,src);c.blit_framebuffer([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixels(&mut c,src,2,2),QUAD);
}

#[test]
fn scaling_inversion_and_linear_filter_use_source_pixel_centers() {
    let mut c=Context::new_webgl2(4,4).unwrap();
    let (src,_,_)=fbo(&mut c,2,2,Some(&QUAD),false);
    bind(&mut c,src,0);c.blit_framebuffer([0,0,2,2],[4,4,0,0],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixel(&mut c,0,0,0),[255,255,255,255]); assert_eq!(pixel(&mut c,0,3,3),[255,0,0,255]);
    bind(&mut c,src,0);c.blit_framebuffer([2,2,0,0],[4,4,0,0],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixel(&mut c,0,0,0),[255,0,0,255]); assert_eq!(pixel(&mut c,0,3,3),[255,255,255,255]);
    bind(&mut c,src,0);c.blit_framebuffer([0,0,2,2],[0,0,3,3],COLOR_BUFFER_BIT,LINEAR).unwrap();
    assert_eq!(pixel(&mut c,0,1,1),[128,128,128,255]); assert_eq!(pixel(&mut c,0,0,0),[255,0,0,255]);
    assert_eq!(pixel(&mut c,0,1,0),[128,128,0,255]);
}

#[test]
fn clipping_preserves_mapping_and_scissor_is_the_only_fragment_state() {
    let mut c=Context::new_webgl2(4,2).unwrap(); let (src,_,_)=fbo(&mut c,2,2,Some(&QUAD),false);
    bind(&mut c,src,0); c.clear_color([0.25,0.25,0.25,1.]);c.clear(COLOR_BUFFER_BIT).unwrap();
    c.viewport(50,50,0,0).unwrap(); c.color_mask([false;4]);c.depth_mask(false);c.enable_depth_test(true);c.depth_func(NEVER).unwrap();
    c.enable_blend(true);c.scissor(1,0,2,1).unwrap();c.enable_scissor(true);
    c.blit_framebuffer([-1,0,3,2],[0,0,4,2],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixel(&mut c,0,0,0),[64,64,64,255]);assert_eq!(pixel(&mut c,0,1,0),[255,0,0,255]);
    assert_eq!(pixel(&mut c,0,2,0),[0,255,0,255]);assert_eq!(pixel(&mut c,0,3,0),[64,64,64,255]);
    assert_eq!(pixel(&mut c,0,1,1),[64,64,64,255]);assert_eq!(c.viewport_rect(),[50,50,0,0]);
    c.enable_scissor(false);bind(&mut c,src,0);
    c.blit_framebuffer([0,0,2,2],[-2,0,2,2],COLOR_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixel(&mut c,0,0,0),[0,255,0,255]);
}

#[test]
fn all_errors_precede_color_writes_and_invalid_rectangles_are_checked() {
    let mut c=Context::new_webgl2(2,2).unwrap();let(src,_,_)=fbo(&mut c,2,2,Some(&QUAD),true);
    bind(&mut c,src,0);let before=c.framebuffer_rgba().to_vec();
    for (s,d,mask,filter,error) in [
        ([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT|DEPTH_BUFFER_BIT,NEAREST,Error::InvalidOperation),
        ([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT|DEPTH_BUFFER_BIT,LINEAR,Error::InvalidOperation),
        ([i32::MIN,0,0,2],[0,0,2,2],COLOR_BUFFER_BIT,NEAREST,Error::InvalidValue),
        ([0,0,2,2],[i32::MAX,0,-1,2],COLOR_BUFFER_BIT,NEAREST,Error::InvalidValue),
        ([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT,0,Error::InvalidEnum),
        ([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT|1,NEAREST,Error::InvalidValue),
    ] {assert_eq!(c.blit_framebuffer(s,d,mask,filter),Err(error));assert_eq!(c.framebuffer_rgba(),before);}
    let incomplete=c.create_framebuffer();bind(&mut c,src,incomplete);
    assert_eq!(c.blit_framebuffer([0;4],[0;4],0,NEAREST),Err(Error::InvalidFramebufferOperation));
    bind(&mut c,incomplete,0);assert_eq!(c.blit_framebuffer([0;4],[0;4],0,NEAREST),Err(Error::InvalidFramebufferOperation));
    let mut one=Context::new(1,1).unwrap();assert_eq!(one.blit_framebuffer([0;4],[0;4],0,NEAREST),Err(Error::InvalidOperation));
}

#[test]
fn identical_images_are_rejected_even_across_fbos_or_nonoverlapping_rectangles() {
    let mut c=Context::new_webgl2(2,2).unwrap();let(src,t,r)=fbo(&mut c,2,2,Some(&QUAD),true);
    let alias=c.create_framebuffer();c.bind_framebuffer(FRAMEBUFFER,alias).unwrap();
    c.framebuffer_texture_2d(FRAMEBUFFER,COLOR_ATTACHMENT0,TEXTURE_2D,t,0).unwrap();
    bind(&mut c,src,alias);
    assert_eq!(c.blit_framebuffer([0,0,1,1],[1,1,2,2],COLOR_BUFFER_BIT,NEAREST),Err(Error::InvalidOperation));
    let (dst,_,_)=fbo(&mut c,2,2,None,false);
    c.framebuffer_renderbuffer(FRAMEBUFFER,DEPTH_ATTACHMENT,RENDERBUFFER,r).unwrap();bind(&mut c,src,dst);
    assert_eq!(c.blit_framebuffer([0,0,1,1],[1,1,2,2],COLOR_BUFFER_BIT|DEPTH_BUFFER_BIT,NEAREST),Err(Error::InvalidOperation));
    assert!(pixels(&mut c,dst,2,2).iter().all(|v|*v==0));
    bind(&mut c,0,0);assert_eq!(c.blit_framebuffer([0,0,1,1],[1,1,2,2],COLOR_BUFFER_BIT,NEAREST),Err(Error::InvalidOperation));
    c.blit_framebuffer([0,0,1,1],[1,1,2,2],0,NEAREST).unwrap();
    c.blit_framebuffer([0,0,1,1],[1,1,2,2],STENCIL_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(c.blit_framebuffer([0,0,1,1],[1,1,2,2],STENCIL_BUFFER_BIT,LINEAR),Err(Error::InvalidOperation));
}

fn triangle(c:&mut Context) {
    let p=c.create_program();
    for (kind,source) in [(VERTEX_SHADER,"attribute vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}"),(FRAGMENT_SHADER,"precision mediump float;void main(){gl_FragColor=vec4(0.0,1.0,0.0,1.0);}")] {
        let s=c.create_shader(kind).unwrap();c.shader_source(s,source).unwrap();c.compile_shader(s).unwrap();assert!(c.shader_compile_status(s).unwrap(),"{}",c.shader_info_log(s).unwrap());c.attach_shader(p,s).unwrap();
    }
    c.link_program(p).unwrap();assert!(c.program_link_status(p).unwrap(),"{}",c.program_info_log(p).unwrap());c.use_program(p).unwrap();
    let b=c.create_buffer();c.bind_buffer(ARRAY_BUFFER,b).unwrap();c.buffer_data(ARRAY_BUFFER,&[-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>()).unwrap();
    let a=c.get_attrib_location(p,"p").unwrap() as u32;c.vertex_attrib_pointer(a,2,FLOAT,false,0,0).unwrap();c.enable_vertex_attrib_array(a,true).unwrap();
}

#[test]
fn depth16_blits_scale_nearest_ignore_depth_mask_and_affect_later_draws() {
    let mut c=Context::new_webgl2(1,1).unwrap();let(src,_,_)=fbo(&mut c,2,1,None,true);
    c.clear_depth(0.25);c.clear(DEPTH_BUFFER_BIT).unwrap();c.enable_scissor(true);c.scissor(1,0,1,1).unwrap();c.clear_depth(0.75);c.clear(DEPTH_BUFFER_BIT).unwrap();c.enable_scissor(false);
    let(dst,_,_)=fbo(&mut c,4,2,None,true);c.clear_depth(0.);c.clear(DEPTH_BUFFER_BIT).unwrap();c.depth_mask(false);
    bind(&mut c,src,dst);c.blit_framebuffer([0,0,2,1],[4,0,0,2],DEPTH_BUFFER_BIT,NEAREST).unwrap();
    triangle(&mut c);c.viewport(0,0,4,2).unwrap();c.enable_depth_test(true);c.draw_arrays(TRIANGLES,0,3).unwrap();
    assert_eq!(pixel(&mut c,dst,0,0),[0,255,0,255]);assert_eq!(pixel(&mut c,dst,3,0),[0,0,0,0]);
    assert!(c.framebuffer_rgba().iter().all(|v|*v==0));
}

#[test]
fn missing_depth_destinations_are_ignored_but_missing_sources_are_errors() {
    let mut c=Context::new_webgl2(1,1).unwrap();let(src,_,_)=fbo(&mut c,2,2,Some(&QUAD),false);
    let(dst,_,_)=fbo(&mut c,2,2,None,true);bind(&mut c,src,dst);
    assert_eq!(c.blit_framebuffer([0,0,2,2],[0,0,2,2],COLOR_BUFFER_BIT|DEPTH_BUFFER_BIT,NEAREST),Err(Error::InvalidOperation));
    assert!(pixels(&mut c,dst,2,2).iter().all(|v|*v==0));
    bind(&mut c,dst,src);c.blit_framebuffer([0,0,2,2],[0,0,2,2],DEPTH_BUFFER_BIT|STENCIL_BUFFER_BIT,NEAREST).unwrap();
    c.set_depth_channel(false);bind(&mut c,dst,0);c.blit_framebuffer([0,0,2,2],[0,0,1,1],DEPTH_BUFFER_BIT,NEAREST).unwrap();
    assert_eq!(pixels(&mut c,src,2,2),QUAD);
}

#[test]
fn opaque_default_alpha_and_attachment_reallocation_are_respected() {
    let mut c=Context::new_webgl2(1,1).unwrap();c.set_alpha_channel(false);
    let(src,t,_)=fbo(&mut c,1,1,Some(&[11,22,33,44]),false);bind(&mut c,src,0);
    c.blit_framebuffer([0,0,1,1],[0,0,1,1],COLOR_BUFFER_BIT,NEAREST).unwrap();assert_eq!(pixel(&mut c,0,0,0),[11,22,33,255]);
    bind(&mut c,0,src);c.blit_framebuffer([0,0,1,1],[0,0,1,1],COLOR_BUFFER_BIT,NEAREST).unwrap();assert_eq!(pixel(&mut c,src,0,0),[11,22,33,255]);
    c.bind_texture(TEXTURE_2D,t).unwrap();c.tex_image_2d(TEXTURE_2D,0,RGBA,0,0,0,RGBA,UNSIGNED_BYTE,None).unwrap();bind(&mut c,src,0);
    assert_eq!(c.blit_framebuffer([0,0,1,1],[0,0,1,1],COLOR_BUFFER_BIT,NEAREST),Err(Error::InvalidFramebufferOperation));
}

#[test]
fn extreme_valid_rectangles_are_bounded_by_attachment_and_preserve_exact_clipping() {
    let mut c=Context::new_webgl2(2,2).unwrap();let(src,_,_)=fbo(&mut c,2,2,Some(&QUAD),false);bind(&mut c,src,0);
    c.blit_framebuffer([-1,0,i32::MAX-1,2],[-1,0,i32::MAX-1,2],COLOR_BUFFER_BIT,NEAREST).unwrap();assert_eq!(pixels(&mut c,0,2,2),QUAD);
    bind(&mut c,src,0);c.blit_framebuffer([0,0,2,2],[i32::MIN,0,-1,2],COLOR_BUFFER_BIT,LINEAR).unwrap();assert_eq!(pixels(&mut c,0,2,2),QUAD);
}
