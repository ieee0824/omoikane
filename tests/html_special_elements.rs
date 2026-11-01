//! Dedicated HTML and MathML element interfaces and reflected state.

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn check(script: &str) {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    assert_eq!(runtime.eval(script).unwrap().as_boolean(), Some(true));
}

#[test]
fn special_elements_use_their_dedicated_interfaces() {
    check(
        r#"(() => {
        const constructors = {
          meter: HTMLMeterElement,
          progress: HTMLProgressElement,
          picture: HTMLPictureElement,
          source: HTMLSourceElement,
          track: HTMLTrackElement,
          datalist: HTMLDataListElement,
          map: HTMLMapElement,
          embed: HTMLEmbedElement,
        };
        for (const [name, constructor] of Object.entries(constructors)) {
          const element = document.createElement(name);
          if (!(element instanceof constructor) || element.constructor !== constructor ||
              !(element instanceof HTMLElement)) return false;
        }
        const math = document.createElementNS('http://www.w3.org/1998/Math/MathML', 'math');
        const mi = document.createElementNS('http://www.w3.org/1998/Math/MathML', 'mi');
        const mathAnchor = document.createElementNS('http://www.w3.org/1998/Math/MathML', 'a');
        mathAnchor.href = 'chapter.html'; mathAnchor.target = '_top';
        return math instanceof MathMLElement && mi instanceof MathMLElement &&
          math.constructor === MathMLElement && math instanceof Element &&
          !(math instanceof HTMLElement) && Object.prototype.toString.call(math) === '[object MathMLElement]' &&
          math.dataset === math.dataset && math.style === math.style &&
          typeof math.focus === 'function' && typeof math.blur === 'function' &&
          mathAnchor instanceof MathMLAnchorElement && mathAnchor instanceof MathMLElement &&
          mathAnchor.href.endsWith('/chapter.html') && mathAnchor.target === '_top';
      })()"#,
    );
}

#[test]
fn special_interfaces_survive_parser_clone_and_cross_document_operations() {
    check(
        r#"(() => {
        const host = document.createElement('div');
        host.innerHTML = '<meter></meter><progress></progress><picture><source></picture>' +
          '<track><datalist></datalist><map></map><embed>' +
          '<math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math>';
        const expected = [HTMLMeterElement, HTMLProgressElement, HTMLPictureElement,
          HTMLSourceElement, HTMLTrackElement, HTMLDataListElement, HTMLMapElement,
          HTMLEmbedElement, MathMLElement, MathMLElement];
        const parsed = [host.children[0], host.children[1], host.children[2],
          host.children[2].firstElementChild, host.children[3], host.children[4],
          host.children[5], host.children[6], host.children[7],
          host.children[7].firstElementChild];
        if (!parsed.every((element, index) => element instanceof expected[index])) return false;

        const clone = host.cloneNode(true);
        if (!(clone.children[0] instanceof HTMLMeterElement) ||
            !(clone.children[7] instanceof MathMLElement)) return false;
        const other = document.implementation.createHTMLDocument('other');
        const imported = other.importNode(host, true);
        if (!(imported.children[1] instanceof HTMLProgressElement) ||
            !(imported.children[7].firstElementChild instanceof MathMLElement)) return false;
        const adopted = other.adoptNode(document.createElement('datalist'));
        return adopted instanceof HTMLDataListElement && adopted.ownerDocument === other;
      })()"#,
    );
}

#[test]
fn meter_and_progress_normalize_reflected_numeric_state_and_labels() {
    check(
        r#"(() => {
        const label = document.createElement('label');
        const meter = document.createElement('meter');
        label.append(meter);
        document.body.append(label);
        if (meter.min !== 0 || meter.max !== 1 || meter.value !== 0 ||
            meter.low !== 0 || meter.high !== 1 || meter.optimum !== 0.5 ||
            meter.labels !== meter.labels || meter.labels[0] !== label) return false;
        meter.setAttribute('max', '');
        if (meter.max !== 1) return false;
        meter.setAttribute('value', '+0.25');
        if (meter.value !== 0.25) return false;
        meter.min = 10; meter.max = 20; meter.value = 30;
        meter.low = 8; meter.high = 14; meter.optimum = 25;
        if (meter.value !== 20 || meter.low !== 10 || meter.high !== 14 ||
            meter.optimum !== 20) return false;
        const progressLabel = document.createElement('label');
        const progress = document.createElement('progress');
        progressLabel.append(progress);
        document.body.append(progressLabel);
        if (progress.max !== 1 || progress.value !== 0 || progress.position !== -1 ||
            progress.labels !== progress.labels || progress.labels[0] !== progressLabel) return false;
        progress.max = 4; progress.value = 5;
        if (progress.value !== 4 || progress.position !== 1) return false;
        progress.max = 0;
        if (progress.max !== 4 || progress.getAttribute('max') !== '4') return false;
        progress.max = -1;
        if (progress.max !== 4 || progress.getAttribute('max') !== '4') return false;
        progress.removeAttribute('value');
        return progress.position === -1;
      })()"#,
    );
}

#[test]
fn source_track_datalist_map_and_embed_reflect_their_idl_attributes() {
    check(
        r#"(() => {
        const source = document.createElement('source');
        source.src = 'image.png'; source.srcset = 'small.png 1x'; source.sizes = '50vw';
        source.media = '(width > 10px)'; source.type = 'image/png'; source.width = 12; source.height = 8;
        if (source.getAttribute('src') !== 'image.png' || !source.src.endsWith('/image.png') ||
            source.width !== 12 || source.height !== 8) return false;
        source.width = 4294967297;
        if (source.width !== 1 || source.getAttribute('width') !== '1') return false;
        source.width = -1;
        if (source.width !== 0 || source.getAttribute('width') !== '0') return false;
        source.setAttribute('width', '2147483648');
        if (source.width !== 0) return false;
        source.setAttribute('width', '  +17px');
        if (source.width !== 17) return false;
        source.srcset = 'bad\ud800 1x';
        if (source.getAttribute('srcset') !== 'bad\ufffd 1x') return false;

        const track = document.createElement('track');
        track.kind = 'captions'; track.src = 'captions.vtt'; track.srclang = 'en';
        track.label = 'English'; track.default = true;
        if (track.kind !== 'captions' || !track.src.endsWith('/captions.vtt') || !track.default ||
            track.readyState !== HTMLTrackElement.NONE ||
            !(track.track instanceof TextTrack)) return false;
        track.setAttribute('kind', 'invalid');
        if (track.kind !== 'metadata') return false;

        const datalist = document.createElement('datalist');
        const options = datalist.options;
        datalist.append(document.createElement('option'));
        if (datalist.options !== options || options.length !== 1 || !(options[0] instanceof HTMLOptionElement)) return false;

        const map = document.createElement('map');
        const areas = map.areas;
        map.name = 'places'; map.append(document.createElement('area'));
        if (map.name !== 'places' || map.areas !== areas || areas.length !== 1 || !(areas[0] instanceof HTMLAreaElement)) return false;

        const embed = document.createElement('embed');
        embed.src = 'plugin.bin'; embed.type = 'application/octet-stream'; embed.width = 320; embed.height = 200;
        return embed.src.endsWith('/plugin.bin') && embed.type === 'application/octet-stream' &&
          embed.width === '320' && embed.height === '200' && embed.getSVGDocument() === null;
      })()"#,
    );
}

#[test]
fn text_track_cues_have_stable_lists_and_deterministic_lifecycle() {
    check(
        r#"(() => {
        const video = document.createElement('video');
        const list = video.textTracks;
        const track = video.addTextTrack('captions', 'English', 'en');
        if (video.textTracks !== list || list.length !== 1 || list[0] !== track ||
            list.getTrackById('missing') !== null || track.cues !== null) return false;
        track.mode = 'showing';
        const first = new VTTCue(1, 3, 'first'); first.id = 'a';
        const second = new VTTCue(2, 4, 'second');
        track.addCue(second); track.addCue(first);
        if (!(track.cues instanceof TextTrackCueList) || track.cues.length !== 2 ||
            track.cues[0] !== first || track.cues.getCueById('a') !== first ||
            first.track !== track || first.getCueAsHTML().textContent !== 'first') return false;
        video.currentTime = 2.5;
        if (track.activeCues.length !== 2 || track.activeCues[0] !== first) return false;
        video.currentTime = 3.5;
        if (track.activeCues.length !== 1 || track.activeCues[0] !== second) return false;
        track.removeCue(second);
        const formatted = new VTTCue(-1, Infinity, '<i>formatted</i>').getCueAsHTML();
        const region = new VTTRegion(); region.width = 42; region.scroll = 'invalid';
        if (track.cues.length !== 1 || second.track !== null) throw new Error('removeCue');
        if (Object.prototype.toString.call(first) !== '[object VTTCue]') throw new Error('brand');
        if (!formatted.childNodes[0] || formatted.childNodes[0].localName !== 'i') throw new Error('cue tree element');
        if (formatted.childNodes[0].textContent !== 'formatted') throw new Error('cue tree text');
        if (region.width !== 42 || region.scroll !== '') throw new Error('region state');
        return true;
      })()"#,
    );
}

#[test]
fn special_element_web_idl_descriptors_match_platform_shape() {
    check(
        r#"(() => {
        const interfaces = {
          HTMLMeterElement: ['value','min','max','low','high','optimum','labels'],
          HTMLProgressElement: ['value','max','position','labels'],
          HTMLPictureElement: [],
          HTMLSourceElement: ['src','type','srcset','sizes','media','width','height'],
          HTMLTrackElement: ['kind','src','srclang','label','default','readyState','track'],
          HTMLDataListElement: ['options'], HTMLMapElement: ['name','areas'],
          HTMLEmbedElement: ['src','type','width','height','getSVGDocument'],
          MathMLElement: ['nonce','autofocus','focus','blur','tabIndex','dataset','style'],
          MathMLAnchorElement: ['href','target'],
        };
        for (const [name, members] of Object.entries(interfaces)) {
          const ctor = globalThis[name];
          if (typeof ctor !== 'function' || ctor.length !== 0 ||
              Object.prototype.toString.call(ctor.prototype) !== `[object ${name}]`) return false;
          for (const member of members) {
            if (!Object.getOwnPropertyDescriptor(ctor.prototype, member)?.enumerable) return false;
          }
        }
        for (const name of ['NONE','LOADING','LOADED','ERROR']) {
          for (const owner of [HTMLTrackElement, HTMLTrackElement.prototype]) {
            const descriptor = Object.getOwnPropertyDescriptor(owner, name);
            if (!descriptor || !descriptor.enumerable || descriptor.writable || descriptor.configurable) return false;
          }
        }
        return true;
      })()"#,
    );
}
