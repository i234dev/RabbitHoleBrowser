function make(prefix,start) { var n=start; return function step(){ for(var j=0;j<2;j++){} return prefix+':'+(++n); }; }
var left=make('L',0), right=make('R',10);
var answer=[left(),right(),left(),right()].join('|');
if(answer!=='L:1|R:11|L:2|R:12') throw new Error('closure scope: '+answer);
answer
