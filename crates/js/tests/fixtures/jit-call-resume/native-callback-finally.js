var log=[];
function leaf(n){for(var j=0;j<2;j++){}if(n===2)throw 'two';return n*3;}
function invoke(n){try{return [n].map(leaf)[0];}catch(e){return e;}finally{log.push(n);}}
var answer=[invoke(0),invoke(1),invoke(2),invoke(3)].join(',')+'|'+log.join(',');
if(answer!=='0,3,two,9|0,1,2,3')throw new Error('callback/finally: '+answer);
answer
