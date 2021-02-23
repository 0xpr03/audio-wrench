# Niche audio player

> You want badly implemented music-player ?
> You're tired of full featured players playing songs twice randomly ?
> You just want to continue where it left when you closed it ?
> You like bad UI mockups ?

Pretty basic musicplayer with the following workflow:
- Drop a playlist inside, it'll play it randomly
- Re-Open the program and it'll continue, progress for each playlist is stored internally
- Trash a song while played or favorite it, export favorites as playlist

Only supported files are (based on rodio) mp3,wav,vorbis and flac. mp3-VBR has no track length.

It's accidentally a pure-rust implementation as libvlc and gstreamer are painfully to compile with on windows.

# running
Get [rustc](https://rust-lang.org) run `cargo run` or `cargo run --release`.