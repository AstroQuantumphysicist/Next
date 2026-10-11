//! Offline development pack/sign/verify tool. No production keys are embedded.
use ed25519_dalek::{Signer, SigningKey};
use next_package::{self as pkg, TrustAnchor, VerifiedPackage};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
fn read_bounded(path: &str, max: usize) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > max as u64 {
        return Err("input must be a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("input grew beyond size bound".into());
    }
    Ok(bytes)
}
fn secret(path: &str) -> Result<SigningKey, String> {
    let bytes = read_bounded(path, 32)?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "signing seed must be exactly 32 bytes")?;
    Ok(SigningKey::from_bytes(&seed))
}
fn anchor(key: [u8; 32]) -> TrustAnchor {
    TrustAnchor {
        id: pkg::signer_id(&key),
        key,
        revoked: false,
        allowed_permissions: pkg::KNOWN_PERMISSIONS,
        allowed_kinds: 3,
    }
}
fn w16(b: &mut [u8], at: usize, n: u16) {
    b[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn w32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn w64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
fn run(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str){
        Some("keygen") if args.len()==4=>{
            if Path::new(&args[2]).exists(){let key=secret(&args[2])?.verifying_key().to_bytes();if Path::new(&args[3]).exists(){if read_bounded(&args[3],32)?!=key{return Err("existing public key does not match private seed".into());}}else{OpenOptions::new().write(true).create_new(true).mode(0o644).open(&args[3]).map_err(|e|e.to_string())?.write_all(&key).map_err(|e|e.to_string())?;}return Ok(());}
            let mut seed=[0;32];File::open("/dev/urandom").map_err(|e|e.to_string())?.read_exact(&mut seed).map_err(|e|e.to_string())?;
            let key=SigningKey::from_bytes(&seed).verifying_key().to_bytes();
            OpenOptions::new().write(true).create_new(true).mode(0o600).open(&args[2]).map_err(|e|e.to_string())?.write_all(&seed).map_err(|e|e.to_string())?;
            OpenOptions::new().write(true).create_new(true).mode(0o644).open(&args[3]).map_err(|e|e.to_string())?.write_all(&key).map_err(|e|e.to_string())?;
        }
        Some("verify") if args.len()==4=>{
            let key:[u8;32]=read_bounded(&args[2],32)?.try_into().map_err(|_|"public key must be exactly 32 bytes")?;
            let bytes=read_bounded(&args[3],pkg::MAX_BUNDLE_BYTES)?;
            let package=VerifiedPackage::verify(&bytes,&[anchor(key)]).map_err(|e|format!("package rejected: {e:?}"))?;
            println!("Verified {} {:?} {} files; permissions requested={:#x}",package.id(),package.kind,package.len(),package.permissions);
        }
        Some("install"|"update") if args.len()==5 => {
            let public:[u8;32]=read_bounded(&args[2],32)?.try_into().map_err(|_|"public key must be exactly 32 bytes")?;
            let bytes=read_bounded(&args[4],pkg::MAX_BUNDLE_BYTES)?;
            let result=pkg::installer::Installer::install_at(Path::new(&args[3]),&bytes,&[anchor(public)],args[1]=="update");
            result.map_err(|e|format!("installer: {e:?}"))?;
            println!("{} committed; declared permissions remain requests",args[1]);
        }
        Some("rollback") if args.len()==5 => {
            let public:[u8;32]=read_bounded(&args[2],32)?.try_into().map_err(|_|"public key must be exactly 32 bytes")?;
            let installer=pkg::installer::Installer::open(Path::new(&args[3])).map_err(|e|format!("installer: {e:?}"))?;
            installer.rollback(&args[4],&[anchor(public)]).map_err(|e|format!("installer: {e:?}"))?;
            println!("rollback committed after signature revalidation");
        }
        Some("uninstall") if args.len()==4 => {
            let installer=pkg::installer::Installer::open(Path::new(&args[2])).map_err(|e|format!("installer: {e:?}"))?;
            installer.uninstall(&args[3]).map_err(|e|format!("installer: {e:?}"))?;
            println!("uninstall committed; retained deployments remain available for explicit recovery");
        }
        Some("list") if args.len()==3 => {
            let installer=pkg::installer::Installer::open(Path::new(&args[2])).map_err(|e|format!("installer: {e:?}"))?;
            for app in installer.list().map_err(|e|format!("installer: {e:?}"))? {println!("{}",app.id);}
        }
        Some("pack") if args.len()>=10=>{
            let key=secret(&args[2])?;
            let id=&args[3];if id.len()>=64{return Err("package ID too long".into());}
            let version:Vec<u32>=args[4].split('.').map(|s|s.parse::<u32>().map_err(|_|"invalid numeric version".to_string())).collect::<Result<_,_>>()?;
            if version.len()!=3{return Err("version must be MAJOR.MINOR.PATCH".into());}
            let permissions=u64::from_str_radix(args[5].trim_start_matches("0x"),16).map_err(|_|"permissions must be hexadecimal")?;
            let kind=match args[6].as_str(){"app"=>1,"deployment"=>2,_=>return Err("kind must be app or deployment".into())};
            let entry=&args[7];let output=&args[8];
            let mut files=Vec::new();
            for input in &args[9..]{let (path,source)=input.split_once('=').ok_or("files must be ARCHIVE_PATH=SOURCE_FILE")?;if !pkg::valid_path(path){return Err("invalid canonical relative archive path".into());}files.push((path.to_string(),read_bounded(source,pkg::MAX_FILE_BYTES)?));}
            files.sort_by(|a,b|a.0.cmp(&b.0));
            if files.is_empty()||files.len()>pkg::MAX_FILES{return Err("file count outside bounded profile".into());}
            let entry=files.iter().position(|f|&f.0==entry).ok_or("entry executable missing")?;
            let table_end=pkg::HEADER_BYTES+files.len()*pkg::ENTRY_BYTES;
            let total=files.iter().try_fold(table_end+64,|n,f|n.checked_add(f.1.len())).ok_or("bundle size overflow")?;
            if total>pkg::MAX_BUNDLE_BYTES{return Err("bundle exceeds 16 MiB".into());}
            let mut bytes=vec![0;total];bytes[..8].copy_from_slice(pkg::MAGIC);w16(&mut bytes,8,1);w16(&mut bytes,10,62);w32(&mut bytes,12,pkg::HEADER_BYTES as u32);w64(&mut bytes,24,total as u64);w32(&mut bytes,32,files.len() as u32);w32(&mut bytes,36,entry as u32);w64(&mut bytes,40,permissions);
            for(i,n)in version.iter().enumerate(){w32(&mut bytes,48+i*4,*n);}
            let trust=anchor(key.verifying_key().to_bytes());bytes[64..96].copy_from_slice(&trust.id);bytes[96..96+id.len()].copy_from_slice(id.as_bytes());w16(&mut bytes,162,1);w16(&mut bytes,164,kind);
            let mut offset=table_end;
            for(i,(path,data))in files.iter().enumerate(){let at=pkg::HEADER_BYTES+i*pkg::ENTRY_BYTES;bytes[at..at+path.len()].copy_from_slice(path.as_bytes());w64(&mut bytes,at+64,offset as u64);w64(&mut bytes,at+72,data.len() as u64);bytes[at+80..at+112].copy_from_slice(&pkg::sha256(data));bytes[offset..offset+data.len()].copy_from_slice(data);offset+=data.len();}
            let signature=key.sign(&bytes[..table_end]);bytes[total-64..].copy_from_slice(&signature.to_bytes());
            VerifiedPackage::verify(&bytes,&[trust]).map_err(|e|format!("refusing invalid package: {e:?}"))?;
            let temporary=format!("{output}.tmp");fs::write(&temporary,bytes).map_err(|e|e.to_string())?;fs::rename(&temporary,output).map_err(|e|e.to_string())?;
            println!("Signed {} files into {}; permission declarations are not grants",files.len(),output);
        }
        _=>return Err("usage: next-package keygen SEED_FILE PUBLIC_KEY_FILE | verify PUBLIC_KEY_FILE BUNDLE | pack SEED_FILE ID MAJOR.MINOR.PATCH PERMISSIONS_HEX app|deployment ENTRY_PATH OUTPUT PATH=SOURCE ... | install|update PUBLIC_KEY ROOT BUNDLE | rollback PUBLIC_KEY ROOT ID | uninstall ROOT ID | list ROOT".into())
    }
    Ok(())
}
fn main() {
    if let Err(error) = run(&std::env::args().collect::<Vec<_>>()) {
        eprintln!("next-package: {error}");
        std::process::exit(1);
    }
}
