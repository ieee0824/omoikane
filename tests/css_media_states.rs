//! HTML media pseudo-classes follow live playback state rather than attributes.
use omoikane::{cdp::CdpSession, html::TreeBuilder, js::JsRuntime};
use serde_json::json;

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<head></head><body></body>").document()).unwrap()
}

fn string(runtime: &mut JsRuntime, script: &str) -> String {
    runtime
        .eval(script)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

fn page_value(session: &mut CdpSession, expression: &str) -> serde_json::Value {
    let response = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
        )
        .unwrap();
    assert!(response.get("exceptionDetails").is_none(), "{response}");
    response["result"]["value"].clone()
}

#[test]
fn media_selector_grammar_and_initial_states_are_semantic() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
const names=['playing','paused','seeking','buffering','muted','volume-locked','picture-in-picture'];
if(names.some(name=>!CSS.supports('selector(:'+name+')')))return 'support';
if(names.some(name=>CSS.supports('selector(:'+name+'())')))return 'functional';
document.body.innerHTML='<video muted></video><audio></audio><div></div>';
const [video,audio,div]=document.body.children;
if(!video.matches(':paused:muted')||video.matches(':playing,:seeking,:buffering,:volume-locked,:picture-in-picture'))return 'video';
if(!audio.matches(':paused')||audio.matches(':playing,:muted'))return 'audio';
if(div.matches(':paused,:playing,:muted'))return 'non-media';
const svg=document.createElementNS('http://www.w3.org/2000/svg','video');document.body.append(svg);
if(svg.matches(':paused'))return 'foreign';
return typeof __omoikane_set_media_selector_state==='undefined'?'ok':'binding';
}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn muted_and_seeking_are_live_states_and_invalidate_dependent_styles() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
const style=document.createElement('style');style.textContent='section{width:10px}section:has(video:muted){width:20px}video+span{height:10px}video:seeking+span{height:30px}';document.head.append(style);
const section=document.createElement('section'),video=document.createElement('video'),span=document.createElement('span');section.append(video,span);document.body.append(section);
if(getComputedStyle(section).width!=='10px'||getComputedStyle(span).height!=='10px')return 'initial';
video.muted=true;if(!video.matches(':muted')||video.defaultMuted||video.hasAttribute('muted')||getComputedStyle(section).width!=='20px')return 'muted';
video.defaultMuted=true;if(!video.muted||!video.hasAttribute('muted'))return 'default-on';
video.muted=false;if(video.matches(':muted')||!video.defaultMuted||!video.hasAttribute('muted')||getComputedStyle(section).width!=='10px')return 'live-off';
video.currentTime=1;if(!video.seeking||!video.matches(':seeking')||getComputedStyle(span).height!=='30px')return 'seeking';
globalThis.mediaProbe={video,span};return 'ok';
}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        string(
            &mut runtime,
            "mediaProbe.video.seeking || mediaProbe.video.matches(':seeking') || getComputedStyle(mediaProbe.span).height!=='10px' ? 'stale' : 'ok'",
        ),
        "ok"
    );
}

#[test]
fn play_and_pause_update_matching_before_their_events() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{const video=document.createElement('video');video.src='data:video/mp4;base64,AA==';document.body.append(video);globalThis.playbackProbe={video,events:[]};for(const type of ['play','playing','pause'])video.addEventListener(type,()=>playbackProbe.events.push(type+':'+video.matches(':playing')));video.play();return video.matches(':paused')?'ok':'early';})()"#,
        ),
        "ok"
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        string(
            &mut runtime,
            "playbackProbe.clone=playbackProbe.video.cloneNode(true); playbackProbe.video.matches(':playing') && !playbackProbe.video.matches(':paused') && playbackProbe.clone.matches(':paused') && !playbackProbe.clone.matches(':playing') && playbackProbe.events.join(',')==='play:true,playing:true' ? 'ok' : JSON.stringify(playbackProbe.events)",
        ),
        "ok"
    );
    assert_eq!(
        string(
            &mut runtime,
            "playbackProbe.video.pause(); !playbackProbe.video.matches(':playing') && playbackProbe.video.matches(':paused') ? 'ok' : 'stale'",
        ),
        "ok"
    );
}

#[test]
fn picture_in_picture_matches_only_the_active_video_and_closes_on_exit() {
    let html = r#"<button style="position:absolute;left:0;top:0;width:80px;height:30px">enter</button><video width="320" height="180"></video><script>video=document.querySelector('video');video.src='data:video/mp4,fixture';pipEvents=[];for(const type of ['enterpictureinpicture','leavepictureinpicture'])video.addEventListener(type,e=>pipEvents.push(type+':'+(e instanceof PictureInPictureEvent)+':'+(e.pictureInPictureWindow instanceof PictureInPictureWindow)));document.querySelector('button').onclick=async()=>{try{const window=await video.requestPictureInPicture();pipResult=window.width+'x'+window.height+':'+video.matches(':picture-in-picture')+':'+(document.pictureInPictureElement===video)}catch(e){pipResult=e.name}}</script>"#;
    let encoded = html
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect::<String>();
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":format!("data:text/html,{encoded}")}),
        )
        .unwrap();
    assert_eq!(
        page_value(
            &mut session,
            "document.pictureInPictureEnabled && video.readyState===4 && !video.matches(':picture-in-picture')"
        ),
        true
    );
    for kind in ["mousePressed", "mouseReleased"] {
        session
            .dispatch(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":5,"y":5,"button":"left","clickCount":1}),
            )
            .unwrap();
    }
    assert_eq!(page_value(&mut session, "pipResult"), "320x180:true:true");
    assert_eq!(
        page_value(
            &mut session,
            "document.exitPictureInPicture(); !video.matches(':picture-in-picture') && document.pictureInPictureElement===null && pipEvents.join(',')==='enterpictureinpicture:true:true,leavepictureinpicture:true:true'"
        ),
        true
    );
}
