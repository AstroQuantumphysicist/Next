from PIL import Image
import math
w,h=640,400
im=Image.new('RGB',(w,h));p=im.load()
for y in range(h):
 for x in range(w):
  u=(x-w*.60)/w;v=(y-h*.48)/h
  glow=math.exp(-((u+.3)**2*4+(v+.35)**2*2))
  base=[10+17*glow,25+45*glow,60+75*glow]
  # Folded original ribbons: smooth perspective, rounded tips and layered highlights.
  for k in range(6,-1,-1):
   cx=.13*math.sin(v*3.8+k*.16)+k*.039+.045*(v*v)
   width=.064+.018*math.cos(v*2.0+k*.22)
   q=(u-cx)/width
   if abs(q)<1 and -.63<v<.62:
    t=(q+1)/2
    light=.30+.54*math.exp(-((t-.12)/.16)**2)+.20*math.sin(t*math.pi)
    shade=(1-.14*k)*(1-.16*abs(v))
    base=[(19+24*light)*shade,(53+130*light)*shade,(135+116*light)*shade]
  p[x,y]=tuple(max(0,min(255,round(c))) for c in base)
open(str(__import__('pathlib').Path(__file__).resolve().parents[1]/'crates/prism/assets/wallpaper.rgb'),'wb').write(im.tobytes())
