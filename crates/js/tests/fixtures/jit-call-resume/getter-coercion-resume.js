var log=[];
function inner(n){for(var i=0;i<2;i++){}return 'inner:'+n;}
var obj={get x(){log.push('get');return inner(3);},valueOf(){log.push('value');return 7;}};
function outer(){var a=obj.x;var b=obj+1;try{({get bad(){throw 'bad';}}).bad;}catch(e){log.push(e);}return a+'|'+b+'|'+inner(4);}
var answer=outer()+'|'+log.join(',');
if(answer!=='inner:3|8|inner:4|get,value,bad')throw new Error('resume pc: '+answer);
answer
