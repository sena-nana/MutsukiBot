if(!self.define){let e,s={};const a=(a,c)=>(a=new URL(a+".js",c).href,s[a]||new Promise((s=>{if("document"in self){const e=document.createElement("script");e.src=a,e.onload=s,document.head.appendChild(e)}else e=a,importScripts(a),s()})).then((()=>{let e=s[a];if(!e)throw new Error(`Module ${a} didn’t register its module`);return e})));self.define=(c,i)=>{const d=e||("document"in self?document.currentScript.src:"")||location.href;if(s[d])return;let f={};const r=e=>a(e,d),t={module:{uri:d},exports:f,require:r};s[d]=Promise.all(c.map((e=>t[e]||r(e)))).then((e=>(i(...e),f)))}}define(["./workbox-600ee3f1"],(function(e){"use strict";e.setCacheNameDetails({prefix:"nonebot-plugin-novelai"}),self.addEventListener("message",(e=>{e.data&&"SKIP_WAITING"===e.data.type&&self.skipWaiting()})),e.clientsClaim(),e.precacheAndRoute([{url:"assets/_plugin-vue_export-helper.cdc0426e.js",revision:"25e3a5dcaf00fb2b1ba0c8ecea6d2560"},{url:"assets/404.html.95b7de97.js",revision:"a5ccbe44323c0454eb761c1f116277a8"},{url:"assets/404.html.d1984d43.js",revision:"1f73e7f5ab18fb0b3e71d97e62c7d0c5"},{url:"assets/advance.html.d8645a61.js",revision:"852b61d188369403dbdc9c943dc08e6c"},{url:"assets/advance.html.fb8bd5a0.js",revision:"37300002c7426e96d0d06f818276e5af"},{url:"assets/aidraw.html.a0af9b8a.js",revision:"1b3e089d20ec6e5b4250c78c211ef284"},{url:"assets/aidraw.html.a1c8264d.js",revision:"6dfe157fc9d0cb037369c4c5e5172103"},{url:"assets/app.62b07f63.js",revision:"2d32384981fcbc961a679251d31163f9"},{url:"assets/backend.html.2732ae69.js",revision:"447143e69792ad022eddb7162e39f94f"},{url:"assets/backend.html.fbcfd92c.js",revision:"67e9958588cfa5690e659e2225090923"},{url:"assets/config.html.0e3ae696.js",revision:"2c738acb95b7c2d46a96695bdbc1fd2b"},{url:"assets/config.html.6ce32ec8.js",revision:"bd6511166756c8d9ce33792486051b6d"},{url:"assets/extension.html.10731521.js",revision:"622c0f94e301eeab6c99f36ff746315c"},{url:"assets/extension.html.74bd635b.js",revision:"04e987cf55807933875cc0b2ca9ffbdc"},{url:"assets/flowchart.parse.8bc2fcba.js",revision:"93ee4658efd463b82af7bc1b894a96d4"},{url:"assets/index.html.16eb32c9.js",revision:"d0b916f20bc06ebd4b671c8e3fe239f0"},{url:"assets/index.html.2a46abe5.js",revision:"49e3f17699abdc004f981ac6882facc2"},{url:"assets/index.html.2ce797f4.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.2d711bbc.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.359935e1.js",revision:"874e4a9a8ae85c7835bcb4b227498a5f"},{url:"assets/index.html.454bbb1b.js",revision:"732f8d9309c1fa03f37637315de4f231"},{url:"assets/index.html.4599b5e4.js",revision:"bd917088223ed1c990bbeafc22e9202c"},{url:"assets/index.html.4f0b0940.js",revision:"db99b98b7937feeadf9a8b1641317054"},{url:"assets/index.html.6271dae4.js",revision:"32d6aee496f937dd6d59ce2286386d82"},{url:"assets/index.html.6721e646.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.6e5d616b.js",revision:"91792023eb2dd2884564d5aca362f74f"},{url:"assets/index.html.75a95bb3.js",revision:"f06e5bf59bb94de9f5a1dc4db56ef02f"},{url:"assets/index.html.78cfbaff.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.845c4b13.js",revision:"0d7910f12365e95659cb7ea8d2b9f226"},{url:"assets/index.html.9b1b060f.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.9b2e7e1f.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.ae325769.js",revision:"5976771d2e1d62a4ab0d507e7ece9c82"},{url:"assets/index.html.b33a5889.js",revision:"4ba6811bd7620f38bc470ad8f273624c"},{url:"assets/index.html.b64ab821.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.c762efa7.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.cff9ca04.js",revision:"f1d149fa2758366c8693ca08a36ecc8f"},{url:"assets/index.html.e739f66d.js",revision:"3772923a0ba3609877ec71c1ea8eab63"},{url:"assets/manager.html.97747972.js",revision:"e21adfec9ab5ed368558393bccfc927b"},{url:"assets/manager.html.d3223c0c.js",revision:"8b9bfa3928df3ef9ba4962b34507abca"},{url:"assets/photoswipe.esm.c6c18d70.js",revision:"58c8e5a3e1981882b36217b62f1c7bae"},{url:"assets/search.0782d0d1.svg",revision:"83621669651b9a3d4bf64d1a670ad856"},{url:"assets/style.ab1e2c77.css",revision:"7884e327988a0d756067411eff9d7796"},{url:"index.html",revision:"ebe9196867458b129d8874d177cabaa0"},{url:"404.html",revision:"b663abb21c381878bfff6aefd2df7c05"}],{}),e.cleanupOutdatedCaches()}));
//# sourceMappingURL=service-worker.js.map
