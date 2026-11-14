function fast(x){for(var i=0;i<2;i++){}return 'v'+x;}
function bridge(x){eval('');return fast(x);}
function* gen(){yield bridge(1);return bridge(2);}
var it=gen(),answer=[it.next().value,it.next().value,it.next().done].join('|');
if(answer!=='v1|v2|true')throw new Error('suspension: '+answer);
Promise.resolve(bridge(3)).then(function(v){if(v!=='v3')throw new Error(v);console.log('async:'+v);});
answer
