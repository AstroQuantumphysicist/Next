//! Independent actual-render audit, not guest or accessibility certification.
extern crate prism;
use prism::{appearance::{Accent,Preferences,Theme},client_surface::Previews,palette::Palette,Canvas,PixelFormat};
fn decode(pixel:u32,format:PixelFormat)->u32 {match format {PixelFormat::Bgr=>pixel,PixelFormat::Rgb=>((pixel&255)<<16)|(pixel&0xff00)|((pixel>>16)&255)}}
fn main() {
    let mut failures=0;let mut cases=0;
    for (width,height) in [(640usize,480usize),(1280,800),(1920,1080)] {
        for format in [PixelFormat::Rgb,PixelFormat::Bgr] {
            for theme in [Theme::Light,Theme::Dark,Theme::HighContrast] {
                for accent in [Accent::Blue,Accent::Violet,Accent::Teal,Accent::Orange] {
                    cases+=1;let palette=Palette::new(Preferences{theme,accent,..Preferences::defaults()});
                    let mut previews=Previews::new();let caption=format!("org.example.{}", "m".repeat(52));
                    for slot in 0..2 {previews.submit(slot,1,1,u64::MAX,&caption,1,1,1,format,&[17,34,51,0]).unwrap();}
                    let stride=width+3;let mut pixels=vec![0xdeadbeef;stride*height+2];
                    {let mut canvas=Canvas::new(&mut pixels[1..1+stride*height],width,height,stride,format).unwrap();canvas.set_palette(palette);canvas.clear(0x345678);previews.render(&mut canvas);}
                    assert_eq!(pixels[0],0xdeadbeef);assert_eq!(*pixels.last().unwrap(),0xdeadbeef);
                    assert!((0..height).all(|row|pixels[1+row*stride+width..1+(row+1)*stride].iter().all(|&p|p==0xdeadbeef)));
                    for slot in 0..2 {
                        let b=previews.bounds(slot,width as i32,height as i32).unwrap();let mut escaped=0;
                        for y in b.y as usize..(b.y+88).min(height as i32) as usize {
                            for x in (b.x+b.width) as usize..width {
                                if decode(pixels[1+y*stride+x],format)==palette.text {escaped+=1;}
                            }
                        }
                        if escaped!=0 {failures+=1;println!("FAIL caption escaped preview: {width}x{height} {format:?} {theme:?}/{accent:?} slot{slot} pixels={escaped}");}
                    }
                }
            }
        }
    }
    println!("Actual rendered client caption audit: {cases} combinations, {failures} overflowing preview cards. Host proof only.");
    if failures!=0 {std::process::exit(1);}
}
