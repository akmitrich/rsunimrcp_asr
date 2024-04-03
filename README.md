# rsunimrcp_asr
Rust port of demo UniMRCP recog plugin.
I have plugins for 5 ASR vendors in production. If you are interested please contact me.

## Build
Make sure to satisfy [all the pre-requisits](https://github.com/akmitrich/rsunimrcp-sys#build) for `rsunimrcp-sys` crate.

```bash
$ cargo build --release
```

## Install
Put the file `librsunimrcp_asr.so` into `plugin/` folder of the UniMRCP server installation. And adjust conf file `unimrcpserver.xml` accordingly.
