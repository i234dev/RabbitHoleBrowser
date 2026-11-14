function strict(x){'use strict';for(var i=0;i<2;i++){}return typeof this+':'+x;}
function sloppy(){for(var i=0;i<2;i++){}return Object.prototype.toString.call(this);}
function plain(x){for(var i=0;i<2;i++){}return x+1;}
var bound=strict.bind(7),proxy=new Proxy(plain,{apply(t,th,args){return t(args[0])+1;}});
var answer=[strict.call(7,3),sloppy.call(7),bound(4),proxy(3),Math.abs(-5)].join('|');
if(answer!=='number:3|[object Number]|number:4|5|5')throw new Error('receiver: '+answer);
answer
