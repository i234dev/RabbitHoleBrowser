use rabbithole_webgl::{gl::*, Context};

fn compile(context: &mut Context, kind: u32, source: &str) -> u32 {
    let shader = context.create_shader(kind).unwrap();
    context.shader_source(shader, source).unwrap(); context.compile_shader(shader).unwrap(); shader
}

#[test]
fn local_declarations_and_compound_arithmetic_execute_in_helpers_and_main() {
    for version in [100,300] {
        let (prefix,input,declaration,output) = if version == 300 { ("#version 300 es\n","in","out vec4 pixel;","pixel") } else { ("","attribute","","gl_FragColor") };
        let mut context = if version == 300 { Context::new_webgl2(1,1).unwrap() } else { Context::new(1,1).unwrap() };
        let vertex = compile(&mut context, VERTEX_SHADER, &format!("{prefix}{input} vec2 position;void main(){{vec2 p;p=position;p+=vec2(0.0);gl_Position=vec4(p,0.0,1.0);}}"));
        for body in [
            "vec3 col;bool selected;selected=true;if(selected){col=vec3(0.0,0.25,0.5);}else{col=vec3(1.0);}col+=0.25;",
            "vec3 col=vec3(1.0,2.0,3.0);col*=0.5;col/=2.0;col+=vec3(0.5);col-=vec3(0.5);",
        ] {
            let fragment = compile(&mut context, FRAGMENT_SHADER, &format!("{prefix}precision highp float;{declaration}vec3 adjust(vec3 value){{vec3 local;local=value;local+=vec3(0.0);return local;}}void main(){{{body}{output}=vec4(adjust(col),1.0);}}"));
            for shader in [vertex,fragment] { assert!(context.shader_compile_status(shader).unwrap(),"{}",context.shader_info_log(shader).unwrap()); }
            let program=context.create_program(); for shader in [vertex,fragment] {context.attach_shader(program,shader).unwrap();}
            context.link_program(program).unwrap();assert!(context.program_link_status(program).unwrap(),"{}",context.program_info_log(program).unwrap());context.use_program(program).unwrap();
            let buffer=context.create_buffer();context.bind_buffer(ARRAY_BUFFER,buffer).unwrap();
            let bytes:Vec<_>=[-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect();context.buffer_data(ARRAY_BUFFER,&bytes).unwrap();
            let location=context.get_attrib_location(program,"position").unwrap() as u32;context.vertex_attrib_pointer(location,2,FLOAT,false,0,0).unwrap();context.enable_vertex_attrib_array(location,true).unwrap();
            context.draw_arrays(TRIANGLES,0,3).unwrap();assert_eq!(context.read_pixels(0,0,1,1).unwrap(),[64,128,191,255]);
        }
    }
}

#[test]
fn compound_assignment_retains_lvalue_type_and_grammar_errors() {
    for version in [100,300] {
        let (prefix,declaration,output)=if version==300 {("#version 300 es\n","out vec4 pixel;","pixel")}else{("","","gl_FragColor")};
        let mut context=if version==300 {Context::new_webgl2(1,1).unwrap()}else{Context::new(1,1).unwrap()};
        for body in ["float x+=1.0;", "float x=0.0;x + =1.0;", "bool x=true;x+=false;", "vec3 x=vec3(0.0);x+=vec2(1.0);", "missing+=1.0;", "gain+=1.0;", "float x=1.0;float y=(x+=1.0);"] {
            let shader=compile(&mut context,FRAGMENT_SHADER,&format!("{prefix}precision highp float;uniform float gain;{declaration}void main(){{{body}{output}=vec4(1.0);}}"));
            assert!(!context.shader_compile_status(shader).unwrap(),"{body}");
        }
    }
}
