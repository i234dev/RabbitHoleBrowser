;; Benchmark kernels (also used as an end-to-end test fixture).
(module
  (memory (export "memory") 8)

  ;; Recursive Fibonacci.
  (func $fib (export "fib") (param $n i32) (result i32)
    (if (result i32) (i32.lt_u (local.get $n) (i32.const 2))
      (then (local.get $n))
      (else (i32.add (call $fib (i32.sub (local.get $n) (i32.const 1)))
                     (call $fib (i32.sub (local.get $n) (i32.const 2)))))))

  ;; n x n f64 matrices at 0 (A), n*n*8 (B), 2*n*n*8 (C): C = A * B.
  ;; Fills A[i][j] = i + j, B[i][j] = i - j first; returns the trace of C.
  (func (export "matmul") (param $n i32) (result f64)
    (local $i i32) (local $j i32) (local $k i32) (local $sum f64)
    (local $b i32) (local $c i32) (local $tr f64)
    (local.set $b (i32.mul (i32.mul (local.get $n) (local.get $n)) (i32.const 8)))
    (local.set $c (i32.shl (local.get $b) (i32.const 1)))
    (local.set $i (i32.const 0))
    (loop $fi
      (local.set $j (i32.const 0))
      (loop $fj
        (f64.store
          (i32.shl (i32.add (i32.mul (local.get $i) (local.get $n)) (local.get $j)) (i32.const 3))
          (f64.convert_i32_s (i32.add (local.get $i) (local.get $j))))
        (f64.store
          (i32.add (local.get $b) (i32.shl (i32.add (i32.mul (local.get $i) (local.get $n)) (local.get $j)) (i32.const 3)))
          (f64.convert_i32_s (i32.sub (local.get $i) (local.get $j))))
        (br_if $fj (i32.lt_s (local.tee $j (i32.add (local.get $j) (i32.const 1))) (local.get $n))))
      (br_if $fi (i32.lt_s (local.tee $i (i32.add (local.get $i) (i32.const 1))) (local.get $n))))
    (local.set $i (i32.const 0))
    (loop $li
      (local.set $j (i32.const 0))
      (loop $lj
        (local.set $sum (f64.const 0))
        (local.set $k (i32.const 0))
        (loop $lk
          (local.set $sum
            (f64.add (local.get $sum)
              (f64.mul
                (f64.load (i32.shl (i32.add (i32.mul (local.get $i) (local.get $n)) (local.get $k)) (i32.const 3)))
                (f64.load (i32.add (local.get $b) (i32.shl (i32.add (i32.mul (local.get $k) (local.get $n)) (local.get $j)) (i32.const 3)))))))
          (br_if $lk (i32.lt_s (local.tee $k (i32.add (local.get $k) (i32.const 1))) (local.get $n))))
        (f64.store (i32.add (local.get $c) (i32.shl (i32.add (i32.mul (local.get $i) (local.get $n)) (local.get $j)) (i32.const 3))) (local.get $sum))
        (br_if $lj (i32.lt_s (local.tee $j (i32.add (local.get $j) (i32.const 1))) (local.get $n))))
      (br_if $li (i32.lt_s (local.tee $i (i32.add (local.get $i) (i32.const 1))) (local.get $n))))
    (local.set $i (i32.const 0))
    (loop $t
      (local.set $tr (f64.add (local.get $tr)
        (f64.load (i32.add (local.get $c) (i32.shl (i32.mul (local.get $i) (i32.add (local.get $n) (i32.const 1))) (i32.const 3))))))
      (br_if $t (i32.lt_s (local.tee $i (i32.add (local.get $i) (i32.const 1))) (local.get $n))))
    (local.get $tr))

  ;; SHA-256 compression over `blocks` 64-byte blocks of a deterministic
  ;; message at 65536; the state lives at 1024 and the schedule at 2048.
  ;; Returns the first state word.
  (data (i32.const 0) "\98\2f\8a\42\91\44\37\71\cf\fb\c0\b5\a5\db\b5\e9\5b\c2\56\39\f1\11\f1\59\a4\82\3f\92\d5\5e\1c\ab"
    "\98\aa\07\d8\01\5b\83\12\be\85\31\24\c3\7d\0c\55\74\5d\be\72\fe\b1\de\80\a7\06\dc\9b\74\f1\9b\c1"
    "\c1\69\9b\e4\86\47\be\ef\c6\9d\c1\0f\cc\a1\0c\24\6f\2c\e9\2d\aa\84\74\4a\dc\a9\b0\5c\da\88\f9\76"
    "\52\51\3e\98\6d\c6\31\a8\c8\27\03\b0\c7\7f\59\bf\f3\0b\e0\c6\47\91\a7\d5\51\63\ca\06\67\29\29\14"
    "\85\0a\b7\27\38\21\1b\2e\fc\6d\2c\4d\13\0d\38\53\54\73\0a\65\bb\0a\6a\76\2e\c9\c2\81\85\2c\72\92"
    "\a1\e8\bf\a2\4b\66\1a\a8\70\8b\4b\c2\a3\51\6c\c7\19\e8\92\d1\24\06\99\d6\85\35\0e\f4\70\a0\6a\10"
    "\16\c1\a4\19\08\6c\37\1e\4c\77\48\27\b5\bc\b0\34\b3\0c\1c\39\4a\aa\d8\4e\4f\ca\9c\5b\f3\6f\2e\68"
    "\ee\82\8f\74\6f\63\a5\78\14\78\c8\84\08\02\c7\8c\fa\ff\be\90\eb\6c\50\a4\f7\a3\f9\be\f2\78\71\c6")
  (func $rotr (param i32 i32) (result i32) (i32.rotr (local.get 0) (local.get 1)))
  (func (export "sha256") (param $blocks i32) (result i32)
    (local $i i32) (local $t i32) (local $blk i32)
    (local $a i32) (local $b i32) (local $c i32) (local $d i32)
    (local $e i32) (local $f i32) (local $g i32) (local $h i32)
    (local $t1 i32) (local $t2 i32) (local $w i32)
    ;; message bytes
    (local.set $i (i32.const 0))
    (loop $m
      (i32.store8 (i32.add (i32.const 65536) (local.get $i)) (i32.mul (local.get $i) (i32.const 31)))
      (br_if $m (i32.lt_u (local.tee $i (i32.add (local.get $i) (i32.const 1))) (i32.shl (local.get $blocks) (i32.const 6)))))
    ;; initial state
    (i32.store (i32.const 1024) (i32.const 0x6a09e667))
    (i32.store (i32.const 1028) (i32.const 0xbb67ae85))
    (i32.store (i32.const 1032) (i32.const 0x3c6ef372))
    (i32.store (i32.const 1036) (i32.const 0xa54ff53a))
    (i32.store (i32.const 1040) (i32.const 0x510e527f))
    (i32.store (i32.const 1044) (i32.const 0x9b05688c))
    (i32.store (i32.const 1048) (i32.const 0x1f83d9ab))
    (i32.store (i32.const 1052) (i32.const 0x5be0cd19))
    (local.set $blk (i32.const 0))
    (loop $block
      ;; schedule
      (local.set $t (i32.const 0))
      (loop $w16
        (i32.store (i32.add (i32.const 2048) (i32.shl (local.get $t) (i32.const 2)))
          (i32.load (i32.add (i32.add (i32.const 65536) (i32.shl (local.get $blk) (i32.const 6))) (i32.shl (local.get $t) (i32.const 2)))))
        (br_if $w16 (i32.lt_u (local.tee $t (i32.add (local.get $t) (i32.const 1))) (i32.const 16))))
      (loop $w64
        (local.set $w (i32.load (i32.add (i32.const 2048) (i32.shl (i32.sub (local.get $t) (i32.const 15)) (i32.const 2)))))
        (local.set $t1 (i32.xor (i32.xor (call $rotr (local.get $w) (i32.const 7)) (call $rotr (local.get $w) (i32.const 18))) (i32.shr_u (local.get $w) (i32.const 3))))
        (local.set $w (i32.load (i32.add (i32.const 2048) (i32.shl (i32.sub (local.get $t) (i32.const 2)) (i32.const 2)))))
        (local.set $t2 (i32.xor (i32.xor (call $rotr (local.get $w) (i32.const 17)) (call $rotr (local.get $w) (i32.const 19))) (i32.shr_u (local.get $w) (i32.const 10))))
        (i32.store (i32.add (i32.const 2048) (i32.shl (local.get $t) (i32.const 2)))
          (i32.add (i32.add (local.get $t1) (local.get $t2))
            (i32.add (i32.load (i32.add (i32.const 2048) (i32.shl (i32.sub (local.get $t) (i32.const 16)) (i32.const 2))))
                     (i32.load (i32.add (i32.const 2048) (i32.shl (i32.sub (local.get $t) (i32.const 7)) (i32.const 2)))))))
        (br_if $w64 (i32.lt_u (local.tee $t (i32.add (local.get $t) (i32.const 1))) (i32.const 64))))
      (local.set $a (i32.load (i32.const 1024)))
      (local.set $b (i32.load (i32.const 1028)))
      (local.set $c (i32.load (i32.const 1032)))
      (local.set $d (i32.load (i32.const 1036)))
      (local.set $e (i32.load (i32.const 1040)))
      (local.set $f (i32.load (i32.const 1044)))
      (local.set $g (i32.load (i32.const 1048)))
      (local.set $h (i32.load (i32.const 1052)))
      (local.set $t (i32.const 0))
      (loop $round
        (local.set $t1 (i32.add (i32.add (i32.add (local.get $h)
            (i32.xor (i32.xor (i32.rotr (local.get $e) (i32.const 6)) (i32.rotr (local.get $e) (i32.const 11))) (i32.rotr (local.get $e) (i32.const 25))))
            (i32.xor (i32.and (local.get $e) (local.get $f)) (i32.and (i32.xor (local.get $e) (i32.const -1)) (local.get $g))))
          (i32.add (i32.load (i32.shl (local.get $t) (i32.const 2))) (i32.load (i32.add (i32.const 2048) (i32.shl (local.get $t) (i32.const 2)))))))
        (local.set $t2 (i32.add
          (i32.xor (i32.xor (i32.rotr (local.get $a) (i32.const 2)) (i32.rotr (local.get $a) (i32.const 13))) (i32.rotr (local.get $a) (i32.const 22)))
          (i32.xor (i32.xor (i32.and (local.get $a) (local.get $b)) (i32.and (local.get $a) (local.get $c))) (i32.and (local.get $b) (local.get $c)))))
        (local.set $h (local.get $g))
        (local.set $g (local.get $f))
        (local.set $f (local.get $e))
        (local.set $e (i32.add (local.get $d) (local.get $t1)))
        (local.set $d (local.get $c))
        (local.set $c (local.get $b))
        (local.set $b (local.get $a))
        (local.set $a (i32.add (local.get $t1) (local.get $t2)))
        (br_if $round (i32.lt_u (local.tee $t (i32.add (local.get $t) (i32.const 1))) (i32.const 64))))
      (i32.store (i32.const 1024) (i32.add (i32.load (i32.const 1024)) (local.get $a)))
      (i32.store (i32.const 1028) (i32.add (i32.load (i32.const 1028)) (local.get $b)))
      (i32.store (i32.const 1032) (i32.add (i32.load (i32.const 1032)) (local.get $c)))
      (i32.store (i32.const 1036) (i32.add (i32.load (i32.const 1036)) (local.get $d)))
      (i32.store (i32.const 1040) (i32.add (i32.load (i32.const 1040)) (local.get $e)))
      (i32.store (i32.const 1044) (i32.add (i32.load (i32.const 1044)) (local.get $f)))
      (i32.store (i32.const 1048) (i32.add (i32.load (i32.const 1048)) (local.get $g)))
      (i32.store (i32.const 1052) (i32.add (i32.load (i32.const 1052)) (local.get $h)))
      (br_if $block (i32.lt_u (local.tee $blk (i32.add (local.get $blk) (i32.const 1))) (local.get $blocks))))
    (i32.load (i32.const 1024)))
)
