use rabbithole_webgl::{gl::*, Context, Error};

fn context(version: u32, width: u32) -> Context {
    if version == 300 { Context::new_webgl2(width, 1) } else { Context::new(width, 1) }.unwrap()
}
fn shader(c: &mut Context, kind: u32, source: &str) -> u32 {
    let s = c.create_shader(kind).unwrap();
    c.shader_source(s, source).unwrap();
    c.compile_shader(s).unwrap();
    assert!(c.shader_compile_status(s).unwrap(), "{source}: {}", c.shader_info_log(s).unwrap());
    s
}
fn fragment(version: u32, extra: &str, body: &str) -> String {
    let prefix = if version == 300 { "#version 300 es\n" } else { "" };
    let output = if version == 300 { "out vec4 pixel;" } else { "" };
    format!("{prefix}precision mediump float;{output}{extra}void main(){{{body}}}")
}
fn output(version: u32) -> &'static str {
    if version == 300 { "pixel" } else { "gl_FragColor" }
}
fn scene(c: &mut Context, version: u32, vertex: Option<&str>, fs: &str) -> u32 {
    let prefix = if version == 300 { "#version 300 es\n" } else { "" };
    let attribute = if version == 300 { "in" } else { "attribute" };
    let default = format!("{prefix}{attribute} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}");
    let v = shader(c, VERTEX_SHADER, vertex.unwrap_or(&default));
    let f = shader(c, FRAGMENT_SHADER, fs);
    let p = c.create_program();
    c.attach_shader(p, v).unwrap();
    c.attach_shader(p, f).unwrap();
    c.link_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap(), "{}", c.program_info_log(p).unwrap());
    c.use_program(p).unwrap();
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let data: Vec<_> = [-1f32, -1., 1., -1., -1., 1., 1., 1.].into_iter().flat_map(f32::to_le_bytes).collect();
    c.buffer_data(ARRAY_BUFFER, &data).unwrap();
    let a = c.get_attrib_location(p, "position").unwrap() as u32;
    c.vertex_attrib_pointer(a, 2, FLOAT, false, 0, 0).unwrap();
    c.enable_vertex_attrib_array(a, true).unwrap();
    p
}
fn draw(c: &mut Context) -> Vec<u8> {
    c.draw_arrays(TRIANGLE_STRIP, 0, 4).unwrap();
    c.read_pixels(0, 0, 1, 1).unwrap()
}
fn set(c: &mut Context, dimension: usize, location: i32, transpose: bool, data: &[f32]) -> Result<(), Error> {
    match dimension {
        2 => c.uniform_matrix2fv(location, transpose, data),
        3 => c.uniform_matrix3fv(location, transpose, data),
        4 => c.uniform_matrix4fv(location, transpose, data),
        _ => unreachable!(),
    }
}

#[test]
fn matrix_constructors_and_noncommutative_products_reach_pixels_in_both_versions() {
    for version in [100, 300] {
        for (expression, expected) in [
            ("vec4(mat2(vec2(0.25,0.5),vec2(0.75,1.0))*vec2(1.0,0.0),0.0,1.0)", [64,128,0,255]),
            ("vec4(vec2(1.0,0.0)*mat2(0.25,0.5,0.75,1.0),0.0,1.0)", [64,191,0,255]),
            ("vec4((mat2(1.0,2.0,3.0,4.0)*mat2(0.0,1.0,1.0,0.0))*vec2(0.25,0.0),0.0,1.0)", [191,255,0,255]),
            ("vec4((mat3(1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0)*mat3(0.0,1.0,0.0,0.0,0.0,1.0,1.0,0.0,0.0))*vec3(0.125,0.0,0.0),1.0)", [128,159,191,255]),
            ("vec4(vec3(0.125,0.0,0.0)*mat3(1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0),1.0)", [32,128,223,255]),
            ("vec4(mat3(mat2(0.25,0.5,0.75,1.0))*vec3(1.0,0.0,0.25),1.0)", [64,128,64,255]),
            ("vec4(mat2(mat3(0.25,0.5,0.75,0.0,1.0,0.0,0.1,0.2,0.3))*vec2(0.0,1.0),0.0,1.0)", [0,255,0,255]),
            ("vec4(mat2(mat2(0.25,0.5,0.75,1.0))*vec2(1.0,0.0),0.0,1.0)", [64,128,0,255]),
            ("vec4(mat2(vec3(0.25,0.5,0.75),vec2(0.5,99.0))*vec2(0.0,1.0),0.0,1.0)", [191,128,0,255]),
            ("vec4((((mat2(1.0)+mat2(0.5))*2.0)-mat2(1.0))/4.0*vec2(1.0),0.0,1.0)", [128,128,0,255]),
            ("vec4((1.0-mat3(1.0))*vec3(0.25),1.0)", [128,128,128,255]),
            ("vec4(mat4(mat3(mat2(0.25)))*vec4(1.0,1.0,0.5,1.0))", [64,64,128,255]),
            ("vec4(mat3(mat4(vec4(1.0,0.0,0.0,9.0),vec4(0.25,0.5,0.75,8.0),vec4(0.0,0.0,1.0,7.0),vec4(6.0)))*vec3(0.0,1.0,0.0),1.0)", [64,128,191,255]),
            ("vec4(mat2(mat4(vec4(1.0,2.0,9.0,8.0),vec4(0.25,0.5,7.0,6.0),vec4(3.0),vec4(4.0)))*vec2(0.0,1.0),0.0,1.0)", [64,128,0,255]),
        ] {
            let mut c = context(version, 1);
            let fs = fragment(version, "", &format!("{}={expression};", output(version)));
            scene(&mut c, version, None, &fs);
            assert_eq!(draw(&mut c), expected, "GLSL{version}: {expression}");
        }
    }
}

#[test]
fn matrix_uniform_types_defaults_updates_and_relink_are_real() {
    for version in [100, 300] {
        for (dimension, kind) in [(2,FLOAT_MAT2),(3,FLOAT_MAT3)] {
            let mut c = context(version, 1);
            let n = dimension * dimension;
            let expression = if dimension == 2 { "vec4(matrix*vec2(1.0,0.0),0.0,1.0)" } else { "vec4(matrix*vec3(1.0,0.0,0.0),1.0)" };
            let fs = fragment(version, &format!("uniform mat{dimension} matrix;"), &format!("{}={expression};", output(version)));
            let p = scene(&mut c, version, None, &fs);
            let old = c.get_uniform_location(p, "matrix").unwrap().unwrap();
            assert_eq!(c.get_uniform_kind(p, old).unwrap(), kind);
            let info = c.active_uniforms(p).unwrap();
            assert_eq!((info[0].name.as_str(),info[0].size,info[0].kind), ("matrix",1,kind));
            assert_eq!(c.get_uniform(p, old).unwrap(), vec![0.; n]);
            assert_eq!(draw(&mut c), [0,0,0,255]);
            let mut values = vec![0.; n];
            values[0] = 0.25; values[1] = 0.5;
            if dimension == 3 { values[2] = 0.75; }
            set(&mut c, dimension, old, false, &values).unwrap();
            assert_eq!(c.get_uniform(p, old).unwrap(), values);
            assert_eq!(draw(&mut c), if dimension == 2 { [64,128,0,255] } else { [64,128,191,255] });
            values[0] = 1.; values[1] = 0.;
            if dimension == 3 { values[2] = 0.; }
            set(&mut c, dimension, old, false, &values).unwrap();
            assert_eq!(draw(&mut c), [255,0,0,255]);
            c.link_program(p).unwrap();
            assert!(c.program_link_status(p).unwrap());
            let new = c.get_uniform_location(p, "matrix").unwrap().unwrap();
            assert_eq!(set(&mut c, dimension, old, false, &values), Err(Error::InvalidOperation));
            assert_eq!(c.get_uniform(p, new).unwrap(), vec![0.; n]);
            assert_eq!(draw(&mut c), [0,0,0,255]);
        }
    }
}

#[test]
fn matrix_setter_families_lengths_and_transpose_fail_atomically() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs = fragment(version, "uniform mat2 small;uniform mat3 medium;uniform mat4 large;uniform vec4 ordinary;", &format!("{}=vec4(small*vec2(1.0),0.0,1.0)+vec4(medium*vec3(0.0),0.0)+large*vec4(0.0)+ordinary;", output(version)));
        let p = scene(&mut c, version, None, &fs);
        let ordinary = c.get_uniform_location(p,"ordinary").unwrap().unwrap();
        let original = [0.125,0.25,0.375,0.];
        c.uniform_f32(ordinary,&original).unwrap();
        let before = draw(&mut c);
        assert_eq!(c.uniform_matrix2fv(ordinary,false,&[0.;4]),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,ordinary).unwrap(),original);
        assert_eq!(draw(&mut c),before);
        let locs: Vec<_> = ["small","medium","large"].map(|name| c.get_uniform_location(p,name).unwrap().unwrap()).into_iter().collect();
        for (dimension, location) in [2,3,4].into_iter().zip(locs.iter().copied()) {
            let n = dimension * dimension;
            let good = vec![0.25; n];
            set(&mut c, dimension, location, false, &good).unwrap();
            let pixels = draw(&mut c);
            for (transpose, data, error) in [
                (false,vec![],Error::InvalidValue),
                (false,vec![0.; n-1],Error::InvalidValue),
                (false,vec![0.; n+1],Error::InvalidValue),
                (false,vec![0.; n*2],Error::InvalidOperation),
                (true,vec![0.; n],Error::InvalidValue),
            ] {
                assert_eq!(set(&mut c,dimension,location,transpose,&data), Err(error));
                assert_eq!(c.get_uniform(p,location).unwrap(),good);
                assert_eq!(draw(&mut c),pixels);
            }
            for other in [2,3,4].into_iter().filter(|&other| other!=dimension) {
                assert_eq!(set(&mut c,other,location,false,&vec![0.;other*other]),Err(Error::InvalidOperation));
                assert_eq!(c.get_uniform(p,location).unwrap(),good);
            }
            assert_eq!(c.uniform_f32(location,&good),Err(Error::InvalidOperation));
            assert_eq!(c.uniform_f32_components(location,4,&[0.;4]),Err(Error::InvalidOperation));
            assert_eq!(c.get_uniform(p,location).unwrap(),good);
            set(&mut c,dimension,-1,true,&[]).unwrap();
        }
    }
}

#[test]
fn matrix3_translation_updates_geometry_and_matrix2_color_is_column_major() {
    for version in [100, 300] {
        let mut c = context(version, 8);
        let prefix = if version == 300 { "#version 300 es\n" } else { "" };
        let attribute = if version == 300 { "in" } else { "attribute" };
        let vs = format!("{prefix}{attribute} vec2 position;uniform mat2 shape;uniform mat3 transform;void main(){{vec3 p=transform*vec3(shape*position,1.0);gl_Position=vec4(p.xy,0.0,p.z);}}");
        let fs = fragment(version,"uniform mat2 tint;",&format!("{}=vec4(tint*vec2(0.25,0.5),0.0,1.0);",output(version)));
        let p = scene(&mut c,version,Some(&vs),&fs);
        let shape = c.get_uniform_location(p,"shape").unwrap().unwrap();
        let transform = c.get_uniform_location(p,"transform").unwrap().unwrap();
        let tint = c.get_uniform_location(p,"tint").unwrap().unwrap();
        c.uniform_matrix2fv(shape,false,&[0.5,0.,0.25,1.]).unwrap();
        c.uniform_matrix2fv(tint,false,&[1.,0.5,0.25,1.]).unwrap();
        for (translation,left,right) in [(-0.5,[96,159,0,255],[0,0,255,255]),(0.5,[0,0,255,255],[96,159,0,255])] {
            c.uniform_matrix3fv(transform,false,&[1.,0.,0.,0.,1.,0.,translation,0.,1.]).unwrap();
            c.clear_color(0.,0.,1.,1.);
            c.clear(COLOR_BUFFER_BIT).unwrap();
            c.draw_arrays(TRIANGLE_STRIP,0,4).unwrap();
            assert_eq!(c.read_pixels(1,0,1,1).unwrap(),left);
            assert_eq!(c.read_pixels(6,0,1,1).unwrap(),right);
        }
    }
}

#[test]
fn matrices_work_inside_existing_typed_helpers_without_matrix_parameters() {
    for version in [100,300] {
        let mut c = context(version,1);
        let fs = fragment(version,"vec2 shade(vec2 color){mat2 transform=mat2(0.25,0.5,0.75,1.0);if(transform!=mat2(0.25,0.5,0.75,1.0))return vec2(1.0,0.0);if(mat3(1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0)==mat3(1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,0.0))return vec2(0.0,1.0);return transform*color;}",&format!("{}=vec4(shade(vec2(1.0,0.0)),0.0,1.0);",output(version)));
        scene(&mut c,version,None,&fs);
        assert_eq!(draw(&mut c),[64,128,0,255]);
    }
}

#[test]
fn invalid_matrix_grammar_dimensions_interfaces_and_helpers_are_rejected() {
    for version in [100,300] {
        for expression in ["mat2()","mat2(vec3(1.0))","mat3(1.0,2.0)","mat2(vec4(1.0),1.0)","mat2(mat2(1.0),1.0)","mat2(true)","mat2(1.0)*vec3(1.0)","mat3(1.0)*vec2(1.0)","mat2(1.0)*mat3(1.0)","mat2(1.0)+vec4(1.0)","mat2(1.0).xy","mat2(1.0)[0]","mat2x2(1.0)","normalize(mat2(1.0))","vec4(mat2(1.0))"] {
            let mut c = context(version,1);
            // Matching self-comparison accepts any supported numeric result;
            // a rejected assignment type must not mask an accepted bad form.
            let source = fragment(version,"",&format!("bool invalid=({expression})==({expression});{}=vec4(1.0);",output(version)));
            let s = c.create_shader(FRAGMENT_SHADER).unwrap();
            c.shader_source(s,&source).unwrap();c.compile_shader(s).unwrap();
            assert!(!c.shader_compile_status(s).unwrap(),"accepted {source}");
            assert!(!c.shader_info_log(s).unwrap().is_empty());
        }
        let matrix_input = if version == 300 { "in mat2 invalid;" } else { "varying mat2 invalid;" };
        for extra in [matrix_input,"uniform mat2 invalid[2];","float helper(mat3 x){return 1.0;}","mat2 helper(){return mat2(1.0);}"] {
            let mut c = context(version,1);
            let source = fragment(version,extra,&format!("{}=vec4(1.0);",output(version)));
            let s = c.create_shader(FRAGMENT_SHADER).unwrap();
            c.shader_source(s,&source).unwrap();c.compile_shader(s).unwrap();
            assert!(!c.shader_compile_status(s).unwrap(),"accepted {source}");
        }
    }
}

#[test]
fn matrix_product_work_is_charged_at_each_transitive_helper_call() {
    let mut c = context(300,1);
    let extra = "vec4 costly(vec4 x){mat4 a=mat4(1.0);mat4 b=a*a;mat4 c=b*b;mat4 d=c*c;return d*x;}";
    let body = "pixel=costly(vec4(1.0))+costly(vec4(1.0))+costly(vec4(1.0));";
    let source = fragment(300,extra,body);
    let s = c.create_shader(FRAGMENT_SHADER).unwrap();
    c.shader_source(s,&source).unwrap();c.compile_shader(s).unwrap();
    assert!(!c.shader_compile_status(s).unwrap());
    assert!(c.shader_info_log(s).unwrap().contains("budget"));
}
