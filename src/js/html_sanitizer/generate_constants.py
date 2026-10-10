from pathlib import Path
import re,json,html,hashlib,argparse
parser=argparse.ArgumentParser(description='Extract sanitizer tables from the complete HTML Standard')
parser.add_argument('--standard-file',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
source=args.standard_file.read_text()
HTML='http://www.w3.org/1999/xhtml';SVG='http://www.w3.org/2000/svg';MATH='http://www.w3.org/1998/Math/MathML'
def text(s):return html.unescape(re.sub('<[^>]+>','',s)).strip()
def codes(s):return [text(v) for v in re.findall(r'<code\b[^>]*>(.*?)</code>',s,re.S)]
def name(s,ns):return dict(name=s,namespace=ns)
elements=[];unsafe=[];navigating=[]
for match in re.finditer(r'<dl class="element">(.*?)</dl>',source,re.S):
 block=match.group(1)
 sanitation=re.search(r'<dt>.*?concept-element-sanitization.*?</dt><dd>(.*?)</dd>',block,re.S)
 if not sanitation:continue
 heading=list(re.finditer(r'<h[34]\b[^>]*>(.*?)</h[34]>',source[:match.start()],re.S))[-1].group(1)
 names=codes(heading)
 value=sanitation.group(1)
 if 'href="#sanitizer-category-default"' in value:
  attributes=codes(value.split('navigating-url-attributes')[0])
  for tag in names:elements.append(dict(**name(tag,HTML),attributes=[name(a,None) for a in dict.fromkeys(attributes)]))
 if 'href="#sanitizer-category-unsafe"' in value:
  unsafe.extend(name(tag,HTML) for tag in names)
 if 'navigating-url-attributes' in value:
  attrs=codes(value[value.index('navigating-url-attributes'):])
  navigating.extend([name(tag,HTML),name(a,None)] for tag in names for a in attrs)
start=source.index('id="built-in-safe-default-configuration"')
section=source[start:]
attrs=re.search(r'dom-sanitizerconfig-attributes[^>]*>.*?</dt><dd>(.*?)</dd>',section,re.S).group(1)
globalattrs=[name(a,None) for a in codes(attrs)]
table=re.search(r'<table>(.*?)</table>',section,re.S).group(1)
for row in re.findall(r'<tr>(.*?)</tr>',table,re.S):
 cells=re.findall(r'<td>(.*?)</td>',row,re.S)
 if len(cells)==3:
  ns={'SVG':SVG,'MathML':MATH}[text(cells[1])]
  elements.append(dict(**name(text(cells[0]),ns),attributes=[name(a,None) for a in codes(cells[2])]))
unsafe.extend([name('frame',HTML),name('script',SVG),name('use',SVG)])
navigating.extend([[name('a',SVG),name('href',ns)] for ns in [None,'http://www.w3.org/1999/xlink']])
assert len(elements)>100 and {'script','iframe','object','embed'} <= {n['name'] for n in unsafe}
ids=re.findall(r'<dfn[^>]+id="(handler-(?:window-)?on[a-z]+)"',source)
result={'eventAttributes': sorted({key.split('-')[-1] for key in ids}|{'onbegin','onend','onrepeat'}), 'default':{'elements':elements,'attributes':globalattrs,'processingInstructions':[],'comments':False,'dataAttributes':False,'javascriptURLs':False},'unsafeElements':unsafe,'navigatingAttributes':navigating}
args.output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print('default elements',len(elements),'unsafe',len(unsafe),'navigating',len(navigating),'globals',len(globalattrs))
print('unsafe',unsafe)
print('source sha256',hashlib.sha256(source.encode()).hexdigest())
