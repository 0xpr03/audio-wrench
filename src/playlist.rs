use quick_xml::{Writer, events::{BytesDecl, BytesText}};
use quick_xml::Reader;
use quick_xml::events::{Event, BytesEnd, BytesStart};
use url::Url;
use std::{fs::File, io::{Cursor, Write}, path::Path};
use std::iter;

use crate::prelude::*;

#[test]
fn test() {
    let mut reader = Reader::from_str(include_str!("../tests/test_playlist.xml"));
    let mut buf = Vec::new();
    loop {
        match reader.read_event(&mut buf) {
            Ok(Event::Eof) => {break;},
            Ok(v) => {dbg!(v);},
            Err(e) => {dbg!(e); break;},
        }
    
        // if we don't keep a borrow elsewhere, we can clear the buffer to keep memory usage low
        buf.clear();
    }
}

#[test]
fn test_write() {
    let files = vec![String::from("C:\\asd\\asd.wav"),String::from("D:\\\\asd_asd2ü.mp3")];
    write_playlist(files.iter(),"../tests/test.xspf").unwrap();
}

enum Track<'a> {
    String(&'a String),
    Url(Url),
}

impl<'a> Track<'a> {
    fn as_str(&'a self) -> &'a str{
        match self {
            Track::String(v) => v.as_str(),
            Track::Url(u) => u.as_str(),
        }
    }
}

pub fn write_playlist<'a, I>(files: I,write_file: &str) -> Result<()>
where
    I: Iterator<Item = &'a String> {
    let mut buf = Vec::new();
    let mut writer = Writer::new_with_indent(Cursor::new(&mut buf),b' ',4);
    
    writer.write_event(Event::Decl(BytesDecl::new(b"1.0",Some(b"UTF-8"),None)))?;
    let mut playlist = BytesStart::borrowed_name(b"playlist");
    playlist.push_attribute(("version","1"));
    playlist.push_attribute(("xmlns","http://xspf.org/ns/0/"));
    writer.write_event(Event::Start(playlist))?;
    writer.write_event(Event::Start(BytesStart::borrowed_name(b"title")))?;
    writer.write_event(Event::Text(BytesText::from_plain_str("Audio-Wrench Favorites")))?;
    writer.write_event(Event::End(BytesEnd::borrowed(b"title")))?;
    let titles = BytesStart::borrowed_name(b"trackList");
    writer.write_event(Event::Start(titles))?;
    for f in files {
        let file_url = if f.starts_with("file:///") {
            Track::String(f)
        } else {
            match Url::from_file_path(f) {
                Ok(v) => Track::Url(v),
                Err(_) => {warn!("Ignoring file {} on export. URLs are not supported!",f); continue; },
            }
        };
        writer.write_event(Event::Start(BytesStart::borrowed_name(b"track")))?;
        // TODO: may want to write track length like VLC
        // optional
        // writer.write_event(Event::Start(BytesStart::borrowed_name(b"title")))?;
        // writer.write_event(Event::Text(BytesText::from_plain_str(f.as_str())))?;
        // writer.write_event(Event::End(BytesEnd::borrowed(b"title")))?;
        writer.write_event(Event::Start(BytesStart::borrowed_name(b"location")))?;
        writer.write_event(Event::Text(BytesText::from_plain_str(file_url.as_str())))?;
        writer.write_event(Event::End(BytesEnd::borrowed(b"location")))?;
        writer.write_event(Event::End(BytesEnd::borrowed(b"track")))?;
    }
    writer.write_event(Event::End(BytesEnd::borrowed(b"trackList")))?;
    writer.write_event(Event::End(BytesEnd::borrowed(b"playlist")))?;
    writer.write_event(Event::Eof)?;

    let mut file = File::create(write_file)?;
    file.write_all(&buf)?;
    Ok(())
}