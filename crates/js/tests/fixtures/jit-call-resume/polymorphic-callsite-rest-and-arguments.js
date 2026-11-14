function plain(x){for(var i=0;i<2;i++){}return x+1;}
function rest(...a){return a.length+':'+a[0];}
function args(){return arguments.length+':'+arguments[0];}
function invoke(f,x){for(var i=0;i<2;i++){}return f(x);}
var out=[invoke(plain,1),invoke(plain,2),invoke(String,3),invoke(rest,4),invoke(args,5)];
try{invoke(null,0);}catch(e){out.push(e.name);}
out.push(invoke(plain,7));var answer=out.join('|');
if(answer!=='2|3|3|1:4|1:5|TypeError|8')throw new Error('callsite: '+answer);
answer
