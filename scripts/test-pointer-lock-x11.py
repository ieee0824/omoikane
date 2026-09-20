#!/usr/bin/env python3
"""Exercise the actual Linux GUI in an isolated Xvfb display (requires Pillow)."""

import argparse
import ctypes as C
import http.server
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess as S
import threading
import time

from PIL import Image, ImageChops, ImageGrab

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True, help='GUI executable built with --features gui')
parser.add_argument('--artifacts', type=Path, required=True, help='new directory for logs and original/compressed screenshots')
args = parser.parse_args()
OUT = args.artifacts.resolve()
OUT.mkdir(parents=True, exist_ok=False)
(OUT / 'harness.py').write_bytes(Path(__file__).read_bytes())
FIXTURE = (ROOT / 'tests/fixtures/anonymized-pointer-lock/native.html').read_bytes()
(OUT / 'fixture.html').write_bytes(FIXTURE)
(OUT / 'inputs.json').write_text(json.dumps({
    'revision': S.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
    'binary': str(args.binary.resolve()),
    'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
    'fixture_sha256': hashlib.sha256(FIXTURE).hexdigest(),
}, indent=2) + '\n')
processes = []
logs = []
evidence = []
env = os.environ.copy()
env['OMOIKANE_TRACE_INPUT'] = '1'

def input_events():
    path = OUT / 'browser.log'
    if not path.exists():
        return []
    return [json.loads(line.removeprefix('OMOIKANE_INPUT '))
            for line in path.read_text().splitlines(keepends=True)
            if line.startswith('OMOIKANE_INPUT ') and line.endswith('\n')]

def start(command, name, **kwargs):
    log = (OUT / (name + '.log')).open('w')
    logs.append(log)
    p = S.Popen(command, env=env, stdout=log, stderr=S.STDOUT, **kwargs)
    processes.append(p)
    return p

def command(*args):
    return S.check_output(args, env=env, text=True, timeout=5).strip()

def wait_for(label, probe, predicate, seconds=20):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        last = probe()
        if predicate(last):
            evidence.append({'step': label, 'result': last})
            print(label, json.dumps(last), flush=True)
            return last
        time.sleep(.1)
    raise AssertionError((label, last))

def screenshot(name):
    marker = {'before': (204,217,232), 'locked': (128,201,155),
              'navigated': (248,241,219)}[name]
    deadline = time.monotonic() + 10
    while True:
        im = ImageGrab.grab(xdisplay=env['DISPLAY']).convert('RGB')
        marker_pixels = sum(count for count,color in im.getcolors(im.width*im.height) if color==marker)
        if marker_pixels > 100000 or time.monotonic() >= deadline:
            break
        time.sleep(.1)
    original = OUT / (name + '.original.png')
    compressed = OUT / (name + '.png')
    im.save(original, compress_level=0)
    im.save(compressed, compress_level=9)
    assert Image.open(compressed).tobytes() == im.tobytes()
    evidence.append({'image': name, 'size': im.size, 'original_bytes': original.stat().st_size,
                     'compressed_bytes': compressed.stat().st_size, 'marker_pixels': marker_pixels})
    assert marker_pixels > 100000, ('page was not painted', name, marker_pixels)
    return im

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = FIXTURE
        if self.path == '/next':
            body = (b'<title>PLTEST:NAVIGATED</title>'
                    b'<div style="background:#f8f1db;width:800px;height:400px">'
                    b'<h1>Navigation released pointer lock</h1></div>')
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    read_fd, write_fd = os.pipe()
    xvfb = start(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1600x1000x24',
                  '-nolisten', 'tcp'], 'xvfb', pass_fds=(write_fd,))
    os.close(write_fd)
    assert select.select([read_fd], [], [], 10)[0], 'Xvfb startup timeout'
    with os.fdopen(read_fd) as display_number:
        env['DISPLAY'] = ':' + display_number.readline().strip()
    assert env['DISPLAY'] != ':'
    env['WINIT_UNIX_BACKEND'] = 'x11'
    env['WINIT_X11_SCALE_FACTOR'] = '1'
    start(['openbox'], 'openbox')
    (OUT / 'display.txt').write_text(command('xdpyinfo'))
    x = C.CDLL('libX11.so.6')
    x.XOpenDisplay.argtypes = [C.c_char_p]; x.XOpenDisplay.restype = C.c_void_p
    d = x.XOpenDisplay(env['DISPLAY'].encode())
    assert d
    x.XDefaultRootWindow.argtypes = [C.c_void_p]; x.XDefaultRootWindow.restype = C.c_ulong
    root = x.XDefaultRootWindow(d)
    x.XCreateSimpleWindow.argtypes = [C.c_void_p,C.c_ulong,C.c_int,C.c_int,C.c_uint,C.c_uint,C.c_uint,C.c_ulong,C.c_ulong]
    x.XCreateSimpleWindow.restype = C.c_ulong
    other = x.XCreateSimpleWindow(d,root,1350,100,180,120,0,0,0xffffff)
    x.XStoreName.argtypes = [C.c_void_p,C.c_ulong,C.c_char_p]
    x.XStoreName(d,other,b'Pointer lock focus test')
    x.XMapWindow.argtypes = [C.c_void_p,C.c_ulong]; x.XMapWindow(d,other)
    x.XFlush.argtypes = [C.c_void_p]; x.XFlush(d)
    # Only this private X server is affected. Held-key focus tests must not
    # introduce real auto-repeat presses while checking synthetic key delivery.
    x.XAutoRepeatOff.argtypes = [C.c_void_p]
    x.XAutoRepeatOff(d); x.XFlush(d)
    x.XGrabPointer.argtypes = [C.c_void_p,C.c_ulong,C.c_int,C.c_uint,C.c_int,C.c_int,C.c_ulong,C.c_ulong,C.c_ulong]
    x.XUngrabPointer.argtypes = [C.c_void_p,C.c_ulong]
    def grabbed():
        result = x.XGrabPointer(d,other,0,0,1,1,0,0,0)
        if result == 0:
            x.XUngrabPointer(d,0); x.XFlush(d)
        assert result in (0,1), ('unexpected XGrabPointer',result)
        return result == 1
    class Cursor(C.Structure):
        _fields_ = [('x',C.c_short),('y',C.c_short),('width',C.c_ushort),('height',C.c_ushort),
                    ('xhot',C.c_ushort),('yhot',C.c_ushort),('serial',C.c_ulong),('pixels',C.POINTER(C.c_ulong)),
                    ('atom',C.c_ulong),('name',C.c_char_p)]
    fixes=C.CDLL('libXfixes.so.3')
    fixes.XFixesGetCursorImage.argtypes=[C.c_void_p]
    fixes.XFixesGetCursorImage.restype=C.POINTER(Cursor)
    x.XFree.argtypes=[C.c_void_p]
    def visible():
        ptr=fixes.XFixesGetCursorImage(d)
        assert ptr
        v=any(ptr.contents.pixels[i] >> 24 for i in range(ptr.contents.width*ptr.contents.height))
        x.XFree(ptr)
        return v
    xtst=C.CDLL('libXtst.so.6')
    xtst.XTestFakeRelativeMotionEvent.argtypes=[C.c_void_p,C.c_int,C.c_int,C.c_ulong]
    app_command=[str(args.binary.resolve()),f'http://127.0.0.1:{server.server_port}/']
    app=start(app_command, 'browser')
    def find_window():
        assert app.poll() is None, ('GUI process exited', app.returncode)
        r=S.run(['xdotool','search','--name','^PLTEST:'],env=env,capture_output=True,text=True)
        return r.stdout.splitlines()
    win=wait_for('window created',find_window,bool)[0]
    command('xdotool','windowsize','--sync',win,'1000','700')
    command('xdotool','windowmove','--sync',win,'50','50')
    command('xdotool','windowactivate','--sync',win)
    def geometry(command_name):
        return {key:int(value) for key,value in (line.split('=',1) for line in command_name.splitlines())}
    bounds=geometry(command('xdotool','getwindowgeometry','--shell',win))
    # Translate the client origin directly; WM reparenting adds decorations
    # whose offsets some xdotool versions count twice in getwindowgeometry.
    x.XTranslateCoordinates.argtypes=[C.c_void_p,C.c_ulong,C.c_ulong,C.c_int,C.c_int,C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_ulong)]
    origin_x=C.c_int(); origin_y=C.c_int(); child_window=C.c_ulong()
    assert x.XTranslateCoordinates(d,int(win),root,0,0,C.byref(origin_x),C.byref(origin_y),C.byref(child_window))
    bounds['X']=origin_x.value; bounds['Y']=origin_y.value
    def title(): return command('xdotool','getwindowname',win)
    def state():
        value=title()
        return json.loads(value[7:]) if value.startswith('PLTEST:{') else {}
    wait_for('page ready',state,lambda s: 'locked' in s)
    command('xdotool','mousemove','--sync','--window',win,'150','200')
    wait_for('visible before lock',visible,bool)
    before=screenshot('before')
    command('xdotool','click','1')
    wait_for('click acquires lock',state,lambda s:s.get('locked') and s.get('resolved')==1)
    wait_for('hidden cursor',visible,lambda v:not v)
    xtst.XTestFakeRelativeMotionEvent(d,5000,-4000,0); x.XFlush(d)
    move=wait_for('unbounded relative motion',state,lambda s:s.get('dx',0)>=5000 and s.get('dy',0)<=-4000)
    assert (move['moves'],move['dx'],move['dy']) == (1,5000,-4000), move
    assert (move['x'],move['y']) == (150,200), move
    position=geometry(command('xdotool','getmouselocation','--shell'))
    assert bounds['X'] <= position['X'] < bounds['X']+bounds['WIDTH'], (bounds,position)
    assert bounds['Y'] <= position['Y'] < bounds['Y']+bounds['HEIGHT'], (bounds,position)
    evidence.append({'confined_pointer':position,'window_bounds':bounds})
    for _ in range(2):
        xtst.XTestFakeRelativeMotionEvent(d,12,-9,0)
    x.XFlush(d)
    repeated=wait_for('identical consecutive movement',state,lambda s:s.get('moves',0)>=3)
    assert (repeated['moves'],repeated['dx'],repeated['dy']) == (3,5024,-4018), repeated
    command('xdotool','click','1')
    command('xdotool','click','5')
    buttons=wait_for('locked button and wheel',state,lambda s:s.get('down',0)>0 and s.get('wheel',0)>0)
    assert (buttons['down'],buttons['wheel']) == (1,1), buttons
    locked=screenshot('locked')
    diff=ImageChops.difference(before,locked)
    evidence.append({'image_diff_bbox':diff.getbbox(), 'image_changed_pixels':sum(pixel!=(0,0,0) for pixel in diff.getdata())})
    command('xdotool','key','Escape')
    wait_for('Escape despite preventDefault',state,lambda s:s.get('locked') is False and s.get('changes')==2)
    wait_for('released native grab',grabbed,lambda v:not v)
    wait_for('visible after Escape',visible,bool)
    position=geometry(command('xdotool','getmouselocation','--shell'))
    assert (position['X'],position['Y']) == (bounds['X']+150,bounds['Y']+200), (position,bounds)
    evidence.append({'restored_cursor':position})
    command('xdotool','key','l')
    wait_for('keyboard reacquires lock',state,lambda s:s.get('locked') and s.get('resolved')==2)
    command('xdotool','key','e')
    wait_for('explicit exit releases page lock',state,lambda s:s.get('locked') is False and s.get('changes')==4)
    wait_for('explicit exit restores cursor',visible,bool)
    command('xdotool','key','l')
    wait_for('reacquire for focus loss',state,lambda s:s.get('locked') and s.get('resolved')==3)
    command('xdotool','windowactivate','--sync',str(other))
    wait_for('focus loss releases native grab',grabbed,lambda v:not v)
    wait_for('focus loss releases page lock',state,lambda s:s.get('locked') is False and s.get('changes')==6)
    # Queue focus and one actual key press while the app cannot process them.
    # On resume winit sees L in the keymap and emits both a synthetic press
    # and the queued real press. This makes the old CI race deterministic.
    start_event = len(input_events())
    previous_keys = state()['keys']
    app.send_signal(signal.SIGSTOP)
    try:
        wait_for('browser stopped for queued focus',
                 lambda: next(line for line in Path(f'/proc/{app.pid}/status').read_text().splitlines()
                              if line.startswith('State:')),
                 lambda s: 'T (stopped)' in s)
        command('xdotool','windowactivate','--sync',win)
        command('xdotool','keydown','l')
    finally:
        app.send_signal(signal.SIGCONT)
    queued = wait_for('focus and synthetic/real L press recorded',
        lambda: input_events()[start_event:],
        lambda events: any(e['kind']=='focus' and e['focused'] for e in events)
        and all(any(e['kind']=='key' and e['code']=='KeyL' and e['pressed']
                    and e['synthetic']==synthetic for e in events) for synthetic in (False,True)))
    focus_sequence = next(e['sequence'] for e in queued if e['kind']=='focus' and e['focused'])
    synthetic_sequence = next(e['sequence'] for e in queued if e['kind']=='key'
                              and e['code']=='KeyL' and e['pressed'] and e['synthetic'])
    real_sequence = next(e['sequence'] for e in queued if e['kind']=='key'
                         and e['code']=='KeyL' and e['pressed'] and not e['synthetic'])
    assert focus_sequence < synthetic_sequence < real_sequence, queued
    command('xdotool','keyup','l')
    focused = wait_for('queued L release reaches page',state,
        lambda s: len(s.get('keys',[]))>len(previous_keys) and s['keys'][-1]==['up','l',False])
    assert focused['keys'][len(previous_keys):] == [['down','l',False],['up','l',False]], focused
    wait_for('one acquisition for queued focus and key',state,
             lambda s:s.get('locked') and s.get('resolved')==4 and s.get('changes')==7)

    command('xdotool','key','e')
    wait_for('exit before held-key focus check',state,
             lambda s:s.get('locked') is False and s.get('changes')==8
             and s.get('keys',[])[-1:]==[['up','e',False]])
    command('xdotool','windowactivate','--sync',str(other))
    start_event = len(input_events())
    previous_keys = state()['keys']
    command('xdotool','keydown','l')
    command('xdotool','windowactivate','--sync',win)
    wait_for('held L synthetic press recorded',lambda:input_events()[start_event:],
             lambda events:any(e['kind']=='key' and e['code']=='KeyL' and e['pressed']
                               and e['synthetic'] for e in events))
    command('xdotool','keyup','l')
    held = wait_for('held L real release reaches page',state,
        lambda s:len(s.get('keys',[]))>len(previous_keys) and s['keys'][-1]==['up','l',False])
    assert held['keys'][len(previous_keys):] == [['up','l',False]], held
    assert (held['locked'],held['resolved'],held['changes']) == (False,4,8), held
    wait_for('held-key focus leaves native grab released',grabbed,lambda v:not v)

    command('xdotool','key','l')
    wait_for('reacquire for navigation',state,lambda s:s.get('locked') and s.get('resolved')==5)
    command('xdotool','key','n')
    wait_for('navigation completes',title,lambda s:s=='PLTEST:NAVIGATED')
    wait_for('navigation releases native grab',grabbed,lambda v:not v)
    screenshot('navigated')
    evidence.append({'status':'PASS'})
    print('PASS', flush=True)
except Exception as error:
    evidence.append({'status':'FAIL', 'error':str(error)})
    raise
finally:
    (OUT/'results.json').write_text(json.dumps(evidence,indent=2)+'\n')
    (OUT/'input-events.json').write_text(json.dumps(input_events(),indent=2)+'\n')
    for p in reversed(processes):
        if p.poll() is None:
            p.terminate()
            try: p.wait(timeout=5)
            except S.TimeoutExpired: p.kill(); p.wait()
    server.shutdown()
    for log in logs: log.close()
