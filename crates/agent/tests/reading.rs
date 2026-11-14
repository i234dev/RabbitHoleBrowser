//! Candidate reading/summary contract, isolated from tool dispatch changes.
use rabbithole_agent::{ax, read, refs::RefMap, summary};
use rabbithole_engine::{dom::{Document,NodeId}, net::Url, Engine, Page, Viewport};

const ARTICLE:&str=include_str!("fixtures/reading-article.html");
fn page(html:&str)->Page{
    let mut engine=Engine::new_bundled_fonts_only();engine.scripting=false;
    engine.load_html(html,Url::parse_absolute("https://reader.example.test/article").unwrap(),Viewport{width:1000.0,height:800.0})
}
fn element(page:&Page,id:&str)->NodeId{page.doc.descendants(Document::ROOT).find(|&n|page.doc.attr(n,"id")==Some(id)).unwrap()}
fn ax_tree(page:&Page,refs:&mut RefMap)->ax::Ax{
    ax::build(&[ax::FrameView{page,children:vec![]}],refs,&ax::BuildOptions::default()).0
}

#[test]
fn read_default_keeps_stable_links_but_omits_only_same_page_sup_citations(){
    let page=page(ARTICLE);let mut refs=RefMap::default();
    let guide=element(&page,"guide");let citation=element(&page,"citation");
    let assigned=refs.assign(0,guide,"link","a detailed guide");
    let text=read::read(&page,&mut refs,0);
    assert!(text.contains(&format!("[a detailed guide](@e{assigned})")),"{text}");
    assert!(!text.contains("[1]"),"{text}");assert_eq!(refs.lookup(0,citation),None);
    assert!(text.contains("external note")&&text.contains("another page")&&text.contains("m2"),"{text}");
    assert!(!text.contains(" ,")&&!text.contains(" ;")&&!text.contains(" :")&&!text.contains(" )")&&!text.contains(" ."),"{text}");
    assert!(text.contains("December 1989,"),"{text}");
}

#[test]
fn read_plain_mode_avoids_refs_and_explicit_cites_can_restore_marks(){
    let page=page(ARTICLE);let mut refs=RefMap::default();
    let plain=read::read_with_options(&page,&mut refs,0,false,false);
    assert!(plain.chars().count()<800,"{} chars: {plain}",plain.chars().count());
    assert!(!plain.contains("@e"),"{plain}");assert!(!plain.contains("[1]"));
    assert!(plain.contains("a detailed guide")&&plain.contains("external note"));
    assert_eq!(refs.lookup(0,element(&page,"guide")),None);
    let cited=read::read_with_options(&page,&mut refs,0,true,true);
    assert!(cited.contains("[[1]](@e"),"{cited}");
    let plain_cited=read::read_with_options(&page,&mut refs,0,false,true);
    assert!(plain_cited.contains("[1]")&&!plain_cited.contains("@e"),"{plain_cited}");
}

#[test]
fn read_code_whitespace_backticks_and_blank_lines_are_preserved(){
    let page=page("<!doctype html><main><p>Before <code>a  b , c</code> , after .</p><pre>  alpha  , beta  \n\n\n```\n  end  </pre><p>Next <code>`value`</code>.</p></main>");
    let text=read::read_with_options(&page,&mut RefMap::default(),0,false,false);
    assert!(text.contains("Before `a  b , c`, after."),"{text}");
    assert!(text.contains("````\n  alpha  , beta  \n\n\n```\n  end  \n````"),"{text}");
    assert!(text.contains("`` `value` ``"),"{text}");
}

#[test]
fn relative_sup_anchor_resolved_against_another_base_is_not_a_same_page_citation(){
    let page=page("<!doctype html><base href='/other/'><main><p>Read <sup><a href='#note'>base note</a></sup>, not a citation for this document.</p></main>");
    let text=read::read(&page,&mut RefMap::default(),0);
    assert!(text.contains("[base note](@e"),"{text}");
}

#[test]
fn summary_prioritizes_major_sections_groups_navs_and_counts_preferences(){
    let page=page(ARTICLE);let mut refs=RefMap::default();let tree=ax_tree(&page,&mut refs);
    let mut first_nav=None;tree.visit(&mut |node,_|{if first_nav.is_none()&&node.role=="navigation"{first_nav=node.reference;}});
    let lines=summary::render(&page,&tree,&mut refs,&[]);let text=lines.join("\n");
    assert!(text.chars().count()<900,"{} chars: {text}",text.chars().count());
    assert!(text.contains(&format!("[nav ×5] @e{}",first_nav.unwrap())),"{text}");
    assert!(text.contains("[main] @e")&&text.contains("[search"),"{text}");
    assert!(text.contains("h2 \"Late One\"")&&text.contains("h2 \"Late Two\""),"{text}");
    let outline:Vec<_>=lines.iter().filter(|line|line.trim_start().starts_with('h')&&line.contains(" @e")).collect();
    assert_eq!(outline.len(),12,"{text}");
    assert!(text.find("h3 \"D1\"").unwrap()<text.find("h2 \"Late One\"").unwrap(),"{text}");
    assert!(!text.contains("h3 \"D9\""),"{text}");
    assert!(text.contains("+2 formularios sin campos de texto"),"{text}");
    assert!(text.contains("(GET /search)")&&!text.contains("form \"Theme\"")&&!text.contains("form \"Size\""),"{text}");
}

#[test]
fn summary_submit_only_forms_qualify_and_stops_after_three_forms(){
    let mut html=String::from("<!doctype html><main><h1>Forms</h1></main>");
    for i in 0..5{html.push_str(&format!("<form action='/send{i}'><button>Send{i}</button></form>"));}
    let page=page(&html);let mut refs=RefMap::default();let tree=ax_tree(&page,&mut refs);
    let text=summary::render(&page,&tree,&mut refs,&[]).join("\n");
    assert_eq!(text.matches("(GET /send").count(),3,"{text}");
    assert!(text.contains("+2 more forms"),"{text}");assert!(!text.contains("sin campos de texto"),"{text}");
}

#[test]
fn read_subtree_options_match_plain_citation_policy_without_changing_refs(){
    let page=page(ARTICLE);let mut refs=RefMap::default();
    let root=page.doc.descendants(Document::ROOT).find(|&n|page.doc.is_element(n,"p")).unwrap();
    let text=read::read_subtree_with_options(&page,&mut refs,0,root,false,false);
    assert!(text.contains("a detailed guide")&&!text.contains("@e")&&!text.contains("[1]"),"{text}");
    assert_eq!(refs.lookup(0,element(&page,"guide")),None);
}

#[test]
fn summary_keeps_dialog_actions_and_essential_landmark_refs(){
    let page=page("<!doctype html><dialog open aria-label='Notice'><button>Close</button></dialog><main><h1>Main</h1></main><div role='search' aria-label='Lookup'><input aria-label='Query'></div>");
    let mut refs=RefMap::default();let tree=ax_tree(&page,&mut refs);
    let text=summary::render(&page,&tree,&mut refs,&[]).join("\n");
    assert!(text.starts_with("⚠ [dialog \"Notice\"] @e"),"{text}");
    assert!(text.contains("\"Close\" @e")&&text.contains("[main] @e")&&text.contains("[search \"Lookup\"] @e"),"{text}");
}
