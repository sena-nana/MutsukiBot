if(!self.define){let e,s={};const a=(a,f)=>(a=new URL(a+".js",f).href,s[a]||new Promise((s=>{if("document"in self){const e=document.createElement("script");e.src=a,e.onload=s,document.head.appendChild(e)}else e=a,importScripts(a),s()})).then((()=>{let e=s[a];if(!e)throw new Error(`Module ${a} didn’t register its module`);return e})));self.define=(f,i)=>{const c=e||("document"in self?document.currentScript.src:"")||location.href;if(s[c])return;let d={};const r=e=>a(e,c),t={module:{uri:c},exports:d,require:r};s[c]=Promise.all(f.map((e=>t[e]||r(e)))).then((e=>(i(...e),d)))}}define(["./workbox-cd2e90fd"],(function(e){"use strict";self.addEventListener("message",(e=>{e.data&&"SKIP_WAITING"===e.data.type&&self.skipWaiting()})),e.clientsClaim(),e.precacheAndRoute([{url:"assets/404.html-234cc61f.js",revision:"86bf24051e4368b49491e990a8b4f9e1"},{url:"assets/404.html-8a1a0f00.js",revision:"fa8736deae9a1a5a7be1a6c4ee487d2e"},{url:"assets/advance.html-63d127d6.js",revision:"f099272ee93b1c03547eec29f7c7f834"},{url:"assets/advance.html-ba5e14b1.js",revision:"8807991a35d9253aeb6766619b6b12e9"},{url:"assets/aidraw.html-0e35c734.js",revision:"107e2b1b060499d58d8fdfb6cf1ec8ba"},{url:"assets/aidraw.html-4f4caa9a.js",revision:"c43b1fbbd359762ff575f8a041e6ac3b"},{url:"assets/aidraw.html-a02e5984.js",revision:"2541f85e191f359335df8c62a6d7e02e"},{url:"assets/aidraw.html-f858fa79.js",revision:"2259a5b2877e0d67ef2599ccddc716d9"},{url:"assets/app-4215724c.js",revision:"d05ce95986aecef428a0bc6fc061477d"},{url:"assets/backend.html-049b23cc.js",revision:"0f25d0af88973a012f1a182ceaf1a205"},{url:"assets/backend.html-2f0cd3da.js",revision:"ad8e0546277bb750f442c2213f51cf8f"},{url:"assets/backend.html-9b72a153.js",revision:"a450caec076ab49f6ea6bb44f9817b2d"},{url:"assets/backend.html-c53e5a54.js",revision:"0f25d0af88973a012f1a182ceaf1a205"},{url:"assets/Catalog-8afe1f7c.js",revision:"40bd76b515e522905b24d6c3cf2ca298"},{url:"assets/config.html-1453fe93.js",revision:"3599a74a4a3b6f78dff202c0cd83fc68"},{url:"assets/config.html-1fd5681f.js",revision:"79a1fdf43eb065344f3dcc980e49efd8"},{url:"assets/config.html-34332cf0.js",revision:"a61c0cc5d210783853fdc2982c41c35f"},{url:"assets/config.html-65f5a888.js",revision:"9924b917284559161ceba161a989d91a"},{url:"assets/extension.html-255f8140.js",revision:"b4d1e715e984085e458b107d5526eea5"},{url:"assets/extension.html-4d7af809.js",revision:"4276f74c9b890b7493a4e05cd52a3525"},{url:"assets/extension.html-633bae84.js",revision:"4276f74c9b890b7493a4e05cd52a3525"},{url:"assets/extension.html-8fe91020.js",revision:"45a6579a476d8738b5a9fe2147d0e63d"},{url:"assets/flowchart.parse-0007e96c.js",revision:"5fce68ee48d56167c2948760a4066c2d"},{url:"assets/framework-09f0a46f.js",revision:"965a384eeed320a123ebc07bab208eb5"},{url:"assets/index-70769223.js",revision:"097390f0c66585e8b9e39361bf5f05d1"},{url:"assets/index.html-0b6d7060.js",revision:"b55efb74ef2b6fdd3ef6ff399ccb1774"},{url:"assets/index.html-1fe99d95.js",revision:"3cec2cf74218a3603621d6417af128c7"},{url:"assets/index.html-295317e8.js",revision:"a2b58b8c95462310331b34c5121c2c17"},{url:"assets/index.html-657228dd.js",revision:"b55efb74ef2b6fdd3ef6ff399ccb1774"},{url:"assets/index.html-6b041bd7.js",revision:"60214e300d127dee497929673bb7e98c"},{url:"assets/index.html-6d52ff9f.js",revision:"cdf1805df9a519134133e4b51e84cf3e"},{url:"assets/index.html-98cfc773.js",revision:"4db3d80ac8aac47a4529c15ccb01e1e2"},{url:"assets/index.html-9b1ca0f3.js",revision:"d03ba635c87c667893f901103234c919"},{url:"assets/index.html-b6a6c59e.js",revision:"089573a34b61c24f47b18928686e3a33"},{url:"assets/index.html-be267c3d.js",revision:"07402ff7836fd1f242f550aee718f3f4"},{url:"assets/index.html-bf7630b4.js",revision:"9fdfb646eed917515a582094cbc26393"},{url:"assets/index.html-c7ff8520.js",revision:"eab2d2f48a4ab104c33061e4aa7863f5"},{url:"assets/install.html-a37dafb4.js",revision:"4a56ee3f47c2bc79606a6ac3bfc4d2ac"},{url:"assets/install.html-d395cd2f.js",revision:"fd2b870b37d9ba02f8e0e1070ae6dff9"},{url:"assets/install.html-fb8f01e4.js",revision:"7db1ccde39ae23a889033a5333c9df3b"},{url:"assets/install.html-fd0c1e5a.js",revision:"e6f181571b2fd78fad39d877c300b239"},{url:"assets/manager.html-456e649e.js",revision:"1b979e65f30d1276c8acdeb5bb99ab7f"},{url:"assets/manager.html-458736d2.js",revision:"5086f6eb4c244138f95b152fd7c65ea9"},{url:"assets/manager.html-f3591ff3.js",revision:"8d5f01e2706ed8abe412a2ca960f5f38"},{url:"assets/manager.html-fd92060a.js",revision:"5086f6eb4c244138f95b152fd7c65ea9"},{url:"assets/photoswipe.esm-a9093b7c.js",revision:"e5f2011f608af205681b3a6e1023fab7"},{url:"assets/SearchResult-c3ba2477.js",revision:"aa6512bc094a70034edf9cfb0c9ee476"},{url:"assets/style-58c336de.css",revision:"f35c7a5aefb0645d53ae1034fef6d8af"},{url:"index.html",revision:"052e83110106df0af631a1a5f4bbe40d"},{url:"404.html",revision:"f783a7e0ed8c9197ac462116d3f8e2df"}],{}),e.cleanupOutdatedCaches()}));
//# sourceMappingURL=service-worker.js.map
