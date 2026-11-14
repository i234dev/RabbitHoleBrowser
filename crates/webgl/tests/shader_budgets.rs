use rabbithole_webgl::{gl::*, Context};

fn context(version:u32)->Context {
    if version==300 {Context::new_webgl2(1,1).unwrap()}else{Context::new(1,1).unwrap()}
}
fn prefix(version:u32)->(&'static str,&'static str,&'static str,&'static str) {
    if version==300 {("#version 300 es\n","in","out vec4 pixel;","pixel")}else{("","attribute","","gl_FragColor")}
}
fn compile(c:&mut Context,kind:u32,source:&str)->u32 {
    let shader=c.create_shader(kind).unwrap();c.shader_source(shader,source).unwrap();c.compile_shader(shader).unwrap();shader
}
fn rejected_for_budget(c:&mut Context,source:&str) {
    let shader=compile(c,FRAGMENT_SHADER,source);
    assert!(!c.shader_compile_status(shader).unwrap(),"over-budget shader accepted");
    assert!(c.shader_info_log(shader).unwrap().contains("budget"),"{}",c.shader_info_log(shader).unwrap());
}

#[test]
fn a_shared_twelve_light_material_produces_the_analytic_pixel_in_both_versions() {
    for version in [100,300] {
        let mut c=context(version);
        let (header,input,declaration,output)=prefix(version);
        let vertex=compile(&mut c,VERTEX_SHADER,&format!("{header}{input} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}"));
        let fragment=format!("{header}precision highp float;{declaration}vec3 lighting(vec3 normal,vec3 direction,vec3 tint){{float amount=max(dot(normal,normalize(direction)),0.0);return tint*amount;}}void main(){{vec3 n=vec3(0.0,0.0,1.0);vec3 color=vec3(0.05,0.1,0.2);{}{output}=vec4(color,1.0);}}","color+=lighting(n,vec3(0.0,0.0,1.0),vec3(0.01,0.02,0.03));".repeat(12));
        let fragment=compile(&mut c,FRAGMENT_SHADER,&fragment);
        for shader in [vertex,fragment] {assert!(c.shader_compile_status(shader).unwrap(),"{}",c.shader_info_log(shader).unwrap());}
        let program=c.create_program();for shader in [vertex,fragment] {c.attach_shader(program,shader).unwrap();}c.link_program(program).unwrap();
        assert!(c.program_link_status(program).unwrap(),"{}",c.program_info_log(program).unwrap());c.use_program(program).unwrap();
        let buffer=c.create_buffer();c.bind_buffer(ARRAY_BUFFER,buffer).unwrap();
        let bytes:Vec<_>=[-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect();c.buffer_data(ARRAY_BUFFER,&bytes).unwrap();
        let location=c.get_attrib_location(program,"position").unwrap()as u32;c.vertex_attrib_pointer(location,2,FLOAT,false,0,0).unwrap();c.enable_vertex_attrib_array(location,true).unwrap();
        c.draw_arrays(TRIANGLES,0,3).unwrap();
        // Every normalized light faces +z: ambient + twelve tint terms gives
        // linear (.17,.34,.56), independent of parser cost bookkeeping.
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[43,87,143,255]);
    }
}

#[test]
fn matrix_work_untaken_branches_and_shared_call_graphs_remain_bounded() {
    for version in [100,300] {
        let mut c=context(version);let(header,_,declaration,output)=prefix(version);
        // A balanced tree keeps parser depth small while matrix products do
        // substantial work; this checks more than a count of syntax nesting.
        let mut product="mat4(1.0)".to_string();
        for _ in 0..7 {product=format!("({product}*{product})");}
        let source=format!("{header}precision highp float;{declaration}void main(){{if(false){{mat4 hidden={product};}}{output}=vec4(1.0);}}");
        rejected_for_budget(&mut c,&source);

        // Function bodies share ASTs. Doubling calls must still accumulate
        // transitive evaluation work at every call site, including unused defs.
        let mut helpers="float f0(float value){return value;}".to_string();
        for index in 1..16 {helpers.push_str(&format!("float f{index}(float value){{return f{}(value)+f{}(value);}}",index-1,index-1));}
        let source=format!("{header}precision highp float;{declaration}{helpers}void main(){{{output}=vec4(1.0);}}");
        rejected_for_budget(&mut c,&source);
    }
}
