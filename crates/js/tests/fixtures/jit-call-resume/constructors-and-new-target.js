function Base(){ for(var i=0;i<2;i++){} this.kind='base'; return 7; }
function ObjectReturn(){ for(var i=0;i<2;i++){} this.kind='ignored'; return {kind:'object'}; }
class Bad extends Base { constructor(){ super(); for(var i=0;i<2;i++){} return 1; } }
class Good extends Base { constructor(){ super(); return {kind:'good'}; } }
function C(x){ for(var i=0;i<2;i++){} this.tag=new.target.name+':'+x; }
var Bound=C.bind(null,4), error='missing';try{new Bad();}catch(e){error=e.name;}
var answer=[new Base().kind,new ObjectReturn().kind,error,new Good().kind,new Bound().tag].join('|');
if(answer!=='base|object|TypeError|good|C:4') throw new Error('constructor return: '+answer);
answer
