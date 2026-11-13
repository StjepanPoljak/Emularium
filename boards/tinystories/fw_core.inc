; Page 0: the matmul, attention, KV cache, BFP and sampling: everything
; the inner loops call. Nothing here calls page-1 code.

; ------------------------------------------------------------ random numbers

; xorshift32 state = SEED * 2654435761 + 1 (or 1), as stepper.XorShift32.
RNG_INIT:
	movlw	0xB1
	movwf	A0
	movlw	0x79
	movwf	A1
	movlw	0x37
	movwf	A2
	movlw	0x9E
	movwf	A3
	movlw	SEED
	movwf	B0
	CLRN	B1, 3
	call	UMUL32
	MOVN	A0, B0, 4
	INCN	A0, 4
	movf	A0,W
	iorwf	A1,W
	iorwf	A2,W
	iorwf	A3,W
	btfsc	STATUS,Z
	incf	A0,F
	goto	RNG_STORE

RNG_LOAD:
	bcf	STATUS,IRP
	movlw	RNG
	movwf	FSR
	movf	INDF,W
	movwf	A0
	incf	FSR,F
	movf	INDF,W
	movwf	A1
	incf	FSR,F
	movf	INDF,W
	movwf	A2
	incf	FSR,F
	movf	INDF,W
	movwf	A3
	return

RNG_STORE:
	bcf	STATUS,IRP
	movlw	RNG
	movwf	FSR
	movf	A0,W
	movwf	INDF
	incf	FSR,F
	movf	A1,W
	movwf	INDF
	incf	FSR,F
	movf	A2,W
	movwf	INDF
	incf	FSR,F
	movf	A3,W
	movwf	INDF
	return

XORAB	macro
	movf	B0,W
	xorwf	A0,F
	movf	B1,W
	xorwf	A1,F
	movf	B2,W
	xorwf	A2,F
	movf	B3,W
	xorwf	A3,F
	endm

; A = the next xorshift32 number.
RNG_NEXT:
	call	RNG_LOAD
	clrf	B0			; x ^= x << 13
	MOVN	B1, A0, 3
	variable	v_n
v_n = 0
	while v_n < 5
	ASLN	B0, 4
v_n = v_n + 1
	endw
	XORAB
	MOVN	B0, A2, 2		; x ^= x >> 17
	clrf	B2
	clrf	B3
	LSRN	B0, 4
	XORAB
	MOVN	B0, A0, 4		; x ^= x << 5
v_n = 0
	while v_n < 5
	ASLN	B0, 4
v_n = v_n + 1
	endw
	XORAB
	goto	RNG_STORE

; ------------------------------------------------------------ matmul

; ROWS rows (0 = 256) of the weight stream, NIN inputs each, times the
; activations (offset binary, in ACT), A = SUMAH:SUMAL, exponent EV. Each
; row's result rshift(acc, e + EV - F) goes to the handler MMH in A:
; 0 stores 3 bytes at OPTR (F = 12), 1 adds to x[IDX] (F = 10),
; 2 writes to the SRAM at SR and tracks the maximum (F = 8).
MM:
	clrf	IDX
MM_ROW:
	call	EE_RD
	movwf	SL
	call	EE_RD
	movwf	SH
	call	EE_RD
	movwf	RE
	clrf	ACC0
	clrf	ACC1
	clrf	ACC2
	call	ACT_FIRST
	movf	NIN,W			; inputs 0..95 in bank 2
	movwf	CNT
	movlw	96
	subwf	NIN,W
	btfss	STATUS,C
	goto	MM_SEG1
	movlw	96
	movwf	CNT
MM_SEG1:
	call	MAC_LOOP
	movlw	96			; inputs 96.. in bank 1
	subwf	NIN,W
	btfss	STATUS,C
	goto	MM_CORR
	btfsc	STATUS,Z
	goto	MM_CORR
	movwf	CNT
	bcf	STATUS,IRP
	movlw	0xA0
	movwf	FSR
	call	MAC_LOOP
MM_CORR:
	; acc -= 128 * (S + A), 24 bit
	clrf	A0
	movf	SL,W
	movwf	A1
	movf	SH,W
	movwf	A2
	clrf	A3
	btfsc	SH,7
	decf	A3,F
	movf	SUMAL,W
	addwf	A1,F
	movf	SUMAH,W
	btfsc	STATUS,C
	incfsz	SUMAH,W
	addwf	A2,F
	btfsc	STATUS,C
	incf	A3,F
	ASRN	A0, 4			; (S + A) << 7
	SUBN	ACC0, A0, 3
	MOVN	A0, ACC0, 3
	call	SEXT24
	movf	RE,W			; shift = e + EV - F
	addwf	EV,W
	movwf	T0
	movlw	12
	btfsc	MMH,0
	movlw	10
	btfsc	MMH,1
	movlw	8
	subwf	T0,F
	call	SHRR_A
	movf	MMH,W
	btfsc	STATUS,Z
	call	H_STORE
	movf	MMH,W
	xorlw	1
	btfsc	STATUS,Z
	call	H_RESID
	movf	MMH,W
	xorlw	2
	btfsc	STATUS,Z
	call	H_LOGIT
	incf	IDX,F
	decfsz	ROWS,F
	goto	MM_ROW
	return

; CNT products of the next weights and the activations at FSR, added to
; ACC2:ACC1:ACC0, modulo 2^24. The multiply starts from the accumulator's
; low byte, so adding it costs nothing (kernels/gen_row.py).
MAC_LOOP:
	movf	ACC0,W
	movwf	PRODH
	call	EE_RD
	movwf	ACC0
	movf	INDF,W
	incf	FSR,F
	rrf	ACC0,F
	variable	v_m
v_m = 0
	while v_m < 8
	btfsc	STATUS,C
	addwf	PRODH,F
	rrf	PRODH,F
	rrf	ACC0,F
v_m = v_m + 1
	endw
	movf	PRODH,W
	addwf	ACC1,F
	btfsc	STATUS,C
	incf	ACC2,F
	decfsz	CNT,F
	goto	MAC_LOOP
	return

H_STORE:
	movf	OPTR,W
	movwf	FSR
	bcf	STATUS,IRP
	btfsc	FLAGS,OIRP
	bsf	STATUS,IRP
	movf	A0,W
	movwf	INDF
	incf	FSR,F
	movf	A1,W
	movwf	INDF
	incf	FSR,F
	movf	A2,W
	movwf	INDF
	movlw	3
	addwf	OPTR,F
	return

; x[IDX] = sat16(x[IDX] + A)
H_RESID:
	MOVN	B0, A0, 4
	movf	IDX,W
	call	XLOAD
	ADDN	A0, B0, 4
	call	SAT16
	movf	IDX,W
	goto	XSTORE

; A = A clamped to -32768..32767
SAT16:
	movlw	0
	btfsc	A1,7
	movlw	0xFF
	xorwf	A2,W
	btfss	STATUS,Z
	goto	SAT16_OUT
	movf	A2,W
	xorwf	A3,W
	btfsc	STATUS,Z
	return
SAT16_OUT:
	movlw	0xFF
	movwf	A0
	movlw	0x7F
	movwf	A1
	btfss	A3,7
	return
	clrf	A0
	movlw	0x80
	movwf	A1
	return

; Logit to the SRAM at SR; MAXP = max(MAXP, A).
H_LOGIT:
	call	MAX_S
	call	SR_OPEN_WR
	call	SR_WR24
	call	SPI_CLOSE
	goto	SR_ADD3

; MAXP = A if A > MAXP (signed, values within 2^30). Clobbers B.
MAX_S:
	MOVN	B0, MAXP, 4
	SUBN	B0, A0, 4
	btfss	B3,7
	return
	MOVN	MAXP, A0, 4
	return

; ------------------------------------------------------------ activations

; W = i: FSR, IRP at activation i.
ACT_PTR:
	movwf	FSR
	movlw	96
	subwf	FSR,W
	btfss	STATUS,C
	goto	ACT_PTR_LO
	addlw	0xA0
	movwf	FSR
	bcf	STATUS,IRP
	return
ACT_PTR_LO:
	movlw	0x10
	addwf	FSR,F
	bsf	STATUS,IRP
	return

; A = x[IDX] * m (m = SH:SL), signed 32 bit.
XM:
	movf	IDX,W
	call	XLOAD
	MOVN	B0, SL, 2
	call	SMUL16
	MOVN	A0, B0, 4
	return

; Stores W (an offset activation) at ACT[IDX] and adds it to SUMA.
ACT_PUT:
	movwf	T1
	movf	IDX,W
	call	ACT_PTR
	movf	T1,W
	movwf	INDF
	addwf	SUMAL,F
	btfsc	STATUS,C
	incf	SUMAH,F
	return

; ------------------------------------------------------------ KV cache

; In place: the 8 values (24 bit) at XT (IRP as set) become their 8 BFP
; mantissas; BFPE = their exponent (F = 12).
BFP_ARR8:
	call	BFP_RESET
	clrf	IDX
BFP_ARR8_TRACK:
	call	ARR8_LOAD
	call	BFP_TRACK
	incf	IDX,F
	btfss	IDX,3
	goto	BFP_ARR8_TRACK
	movlw	12
	call	BFP_FINISH
	clrf	IDX
BFP_ARR8_CONV:
	call	ARR8_LOAD
	call	BFP_CONV
	movwf	T1
	movf	IDX,W
	addwf	XT,W
	movwf	FSR
	movf	T1,W
	movwf	INDF
	incf	IDX,F
	btfss	IDX,3
	goto	BFP_ARR8_CONV
	return

; A = value IDX of the array at XT, sign-extended (IRP as set).
ARR8_LOAD:
	movf	IDX,W
	addwf	IDX,W
	addwf	IDX,W
	addwf	XT,W
	movwf	FSR
	movf	INDF,W
	movwf	A0
	incf	FSR,F
	movf	INDF,W
	movwf	A1
	incf	FSR,F
	movf	INDF,W
	movwf	A2
	goto	SEXT24

; SR = the KV cache entry of LAYER, GRP, slot W: ((LAYER * 4 + GRP) * 256
; + slot) * 18.
KV_ADDR:
	movwf	A0
	rlf	LAYER,W
	movwf	A1
	rlf	A1,F
	movlw	0xFC
	andwf	A1,F
	movf	GRP,W
	addwf	A1,F			; A1:A0 = n
	clrf	A2
	MOVN	B0, A0, 3
	ASLN	B0, 3			; 2n
	ASLN	A0, 3
	ASLN	A0, 3
	ASLN	A0, 3
	ASLN	A0, 3			; 16n
	ADDN	A0, B0, 3
	MOVN	SR0, A0, 3
	return

; Stores this position's k and v (BFP) in the KV cache, slot POSL.
KV_WRITE:
	bcf	STATUS,IRP
	movlw	WK
	movwf	XT
	call	BFP_ARR8
	movf	BFPE,W
	movwf	KE
	bsf	STATUS,IRP
	movlw	WV
	movwf	XT
	call	BFP_ARR8
	movf	BFPE,W
	movwf	VE
	movf	POSL,W
	call	KV_ADDR
	call	SR_OPEN_WR
	bcf	STATUS,IRP
	movlw	WK
	call	WRITE8
	movf	KE,W
	call	SPI_WR
	bsf	STATUS,IRP
	movlw	WV
	call	WRITE8
	movf	VE,W
	call	SPI_WR
	goto	SPI_CLOSE

; Writes the 8 bytes at W (IRP as set) to the open SRAM stream.
WRITE8:
	movwf	FSR
	movlw	8
	movwf	CNT
WRITE8_LOOP:
	movf	INDF,W
	call	SPI_WR
	incf	FSR,F
	decfsz	CNT,F
	goto	WRITE8_LOOP
	return

; ------------------------------------------------------------ attention

; Opens the KV cache of LAYER, GRP at slot 0 for reading; IDX:ROWS = the
; number of entries, min(pos + 1, 256).
KV_OPEN:
	movlw	0
	call	KV_ADDR
	call	SR_OPEN_RD
	MOVN	ROWS, NCTXL, 1
	movf	NCTXH,W
	movwf	IDX
	return

; Counts IDX:ROWS down; Z when it reaches 0.
KV_COUNT:
	movlw	1
	subwf	ROWS,F
	btfss	STATUS,C
	decf	IDX,F
	movf	ROWS,W
	iorwf	IDX,W
	return

; Query head HQ of KV head GRP: attention over the cache, output to
; O_AREA (8 x 24 bit at head * 24).
ATTEND:
	movlw	WQ			; qa, qE = bfp8(q of this head), in place
	btfsc	HQ,0
	movlw	WQ + 24
	movwf	XT
	bcf	STATUS,IRP
	call	BFP_ARR8
	movf	BFPE,W
	movwf	QE
	; context: min(pos + 1, 256)
	movf	POSL,W
	addlw	1
	movwf	NCTXL
	clrf	NCTXH
	movf	NCTXL,F			; POSL = 255: 256
	btfsc	STATUS,Z
	incf	NCTXH,F
	movf	POSH,F
	btfsc	STATUS,Z
	goto	ATT_PASS1
	clrf	NCTXL
	movlw	1
	movwf	NCTXH
ATT_PASS1:
	; max score and min v exponent
	clrf	SMAX
	clrf	SMAX+1
	clrf	SMAX+2
	movlw	0xFF			; -2^24
	movwf	SMAX+3
	movlw	127
	movwf	E0
	call	KV_OPEN
ATT1_LOOP:
	call	ATT_SCORE
	call	MAX_S
	movlw	8
	movwf	CNT
ATT1_SKIP:
	call	SPI_RD
	decfsz	CNT,F
	goto	ATT1_SKIP
	call	SPI_RD
	movwf	VE
	movf	E0,W			; E0 = min(E0, vE)
	subwf	VE,W
	movwf	T0
	btfss	T0,7
	goto	ATT1_NEXT
	movf	VE,W
	movwf	E0
ATT1_NEXT:
	call	KV_COUNT
	btfss	STATUS,Z
	goto	ATT1_LOOP
	call	SPI_CLOSE
	; weights p = exp2(s - max), their sum, and sum of pw * v
	clrf	SSL
	clrf	SSH
	bsf	STATUS,IRP
	movlw	AACC
	movwf	FSR
	movlw	32
	movwf	CNT
ATT_CLR:
	clrf	INDF
	incf	FSR,F
	decfsz	CNT,F
	goto	ATT_CLR
	call	KV_OPEN
ATT2_LOOP:
	call	ATT_SCORE
	SUBN	A0, SMAX, 4
	call	EXP2NEG
	movwf	T1
	addwf	SSL,F
	btfsc	STATUS,C
	incf	SSH,F
	bcf	STATUS,IRP		; v mantissas
	movlw	VABUF
	movwf	FSR
	movlw	8
	movwf	CNT
ATT2_V:
	call	SPI_RD
	movwf	INDF
	incf	FSR,F
	decfsz	CNT,F
	goto	ATT2_V
	call	SPI_RD
	movwf	VE
	movf	T1,F
	btfsc	STATUS,Z
	goto	ATT2_NEXT
	; pw = (p << 6) >> (vE - E0)
	clrf	PWL
	movf	T1,W
	movwf	PWH
	LSRN	PWH, 1			; p << 6 = (p << 8) >> 2
	rrf	PWL,F
	LSRN	PWH, 1
	rrf	PWL,F
	movf	E0,W
	subwf	VE,W
	btfsc	STATUS,Z
	goto	ATT2_ACC
	movwf	CNT
ATT2_PW:
	LSRN	PWH, 1
	rrf	PWL,F
	decfsz	CNT,F
	goto	ATT2_PW
ATT2_ACC:
	call	ATT_ACCUM
ATT2_NEXT:
	call	KV_COUNT
	btfss	STATUS,Z
	goto	ATT2_LOOP
	call	SPI_CLOSE
	; out = rshift(acc * R, 26 + E0), R = round(2^32 / S)
	call	DIV_R
	MOVN	SMAX, A0, 4
	bcf	STATUS,C		; SR = O_AREA + head * 24
	rlf	GRP,W
	iorwf	HQ,W			; head = 2 GRP + HQ
	movwf	SR0
	ASLN	SR0, 1
	ASLN	SR0, 1
	ASLN	SR0, 1			; 8 head
	movf	SR0,W
	addwf	SR0,F
	addwf	SR0,F			; 24 head
	movlw	HIGH (O_AREA)
	movwf	SR1
	movlw	UPPER (O_AREA)
	movwf	SR2
	call	SR_OPEN_WR
	clrf	OPTR
ATT_OUT:
	bsf	STATUS,IRP
	rlf	OPTR,W
	movwf	FSR
	rlf	FSR,W
	andlw	0x1C
	addlw	AACC
	movwf	FSR
	movf	INDF,W
	movwf	A0
	incf	FSR,F
	movf	INDF,W
	movwf	A1
	incf	FSR,F
	movf	INDF,W
	movwf	A2
	incf	FSR,F
	movf	INDF,W
	movwf	A3
	MOVN	B0, SMAX, 4
	call	SMUL32
	movf	E0,W
	addlw	26
	movwf	T0
	call	SHRR_B
	call	SR_WR24
	incf	OPTR,F
	btfss	OPTR,3
	goto	ATT_OUT
	goto	SPI_CLOSE

; Reads an entry's k mantissas and exponent from the SRAM stream; A =
; rshift(qa . ka, qE + kE - 8), with qa at XT (bank 1).
ATT_SCORE:
	CLRN	A0, 4
	movlw	8
	movwf	CNT
ATT_SCORE_LOOP:
	call	SPI_RD
	movwf	T1			; ka
	bcf	STATUS,IRP
	movf	XT,W
	movwf	FSR
	movf	CNT,W			; qa[8 - CNT]
	sublw	8
	addwf	FSR,F
	movf	INDF,W
	movwf	T0			; qa
	movf	T1,W
	call	UMUL8			; unsigned product, then the signed one
	movf	T1,W
	btfsc	T0,7
	subwf	B1,F
	movf	T0,W
	btfsc	T1,7
	subwf	B1,F
	clrf	B2
	btfsc	B1,7
	decf	B2,F
	movf	B2,W
	movwf	B3
	ADDN	A0, B0, 4
	decfsz	CNT,F
	goto	ATT_SCORE_LOOP
	call	SPI_RD
	movwf	KE
	movf	QE,W
	addwf	KE,W
	addlw	-8
	movwf	T0
	goto	SHRR_A

; AACC[j] += pw * va[j] for j = 0..7 (va in VABUF, bank 1).
ATT_ACCUM:
	clrf	OPTR
ATT_ACCUM_LOOP:
	bcf	STATUS,IRP
	movf	OPTR,W
	addlw	VABUF
	movwf	FSR
	movf	INDF,W
	movwf	T1			; va
	btfsc	T1,7
	sublw	0			; |va|
	call	UMUL16X8
	btfss	T1,7
	goto	ATT_ACCUM_SIGN
	NEGN	B0, 3
ATT_ACCUM_SIGN:
	clrf	B3
	btfsc	B2,7
	decf	B3,F
	bsf	STATUS,IRP
	rlf	OPTR,W
	movwf	FSR
	rlf	FSR,W
	andlw	0x1C
	addlw	AACC
	movwf	FSR
	movf	B0,W
	addwf	INDF,F
	incf	FSR,F
	movf	B1,W
	btfsc	STATUS,C
	incfsz	B1,W
	addwf	INDF,F
	incf	FSR,F
	movf	B2,W
	btfsc	STATUS,C
	incfsz	B2,W
	addwf	INDF,F
	incf	FSR,F
	movf	B3,W
	btfsc	STATUS,C
	incfsz	B3,W
	addwf	INDF,F
	incf	OPTR,F
	btfss	OPTR,3
	goto	ATT_ACCUM_LOOP
	return

; B2:B1:B0 = PWH:PWL * W, unsigned.
UMUL16X8:
	movwf	B0
	clrf	B1
	clrf	B2
	bcf	STATUS,C
	rrf	B0,F
	variable	v_u
v_u = 0
	while v_u < 8
	btfss	STATUS,C
	goto	$ + 7
	movf	PWL,W
	addwf	B1,F
	movf	PWH,W
	btfsc	STATUS,C
	incfsz	PWH,W
	addwf	B2,F
	rrf	B2,F
	rrf	B1,F
	rrf	B0,F
v_u = v_u + 1
	endw
	return

; ------------------------------------------------------------ BFP from SRAM

; ACT, SUMA, EV = bfp8 of the NIN 24-bit values at SR (F = 12).
BFP_SRAM:
	MOVN	SL, SR0, 3		; keep the start
	call	BFP_RESET
	call	SR_OPEN_RD
	movf	NIN,W
	movwf	IDX
BFP_SRAM_TRACK:
	call	SR_RD24
	call	BFP_TRACK
	decfsz	IDX,F
	goto	BFP_SRAM_TRACK
	call	SPI_CLOSE
	movlw	12
	call	BFP_FINISH
	MOVN	SR0, SL, 3
	call	SR_OPEN_RD
	clrf	SUMAL
	clrf	SUMAH
	clrf	IDX
BFP_SRAM_CONV:
	call	SR_RD24
	call	BFP_CONV
	xorlw	0x80
	call	ACT_PUT
	incf	IDX,F
	movf	IDX,W
	xorwf	NIN,W
	btfss	STATUS,Z
	goto	BFP_SRAM_CONV
	call	SPI_CLOSE
	movf	BFPE,W
	movwf	EV
	return

; ------------------------------------------------------------ sampling

; NEXTL/NEXTH = the next token: min-p sampling as stepper.sample_int, from
; the logits in LG_AREA with their maximum in MAXP.
SAMPLE:
	CLRN	MINN, 4			; sum of p
	call	LG_OPEN
SAMPLE_SUM:
	call	SAMPLE_P
	addwf	MINN,F
	btfss	STATUS,C
	goto	SAMPLE_SUM_NEXT
	incf	MINN+1,F
	btfsc	STATUS,Z
	incf	MINN+2,F
SAMPLE_SUM_NEXT:
	call	KV_COUNT
	btfss	STATUS,Z
	goto	SAMPLE_SUM
	call	SPI_CLOSE
	call	RNG_NEXT		; r = rng % sum
	call	MOD24
	MOVN	SL, B0, 3
	CLRN	ACC0, 3			; cumulative sum
	clrf	NEXTL
	clrf	NEXTH
	call	LG_OPEN
SAMPLE_PICK:
	call	SAMPLE_P
	addwf	ACC0,F
	btfss	STATUS,C
	goto	SAMPLE_CMP
	incf	ACC1,F
	btfsc	STATUS,Z
	incf	ACC2,F
SAMPLE_CMP:
	movf	ACC0,W			; cum > r: done
	subwf	SL,W
	movf	ACC1,W
	btfss	STATUS,C
	incfsz	ACC1,W
	subwf	SH,W
	movf	ACC2,W
	btfss	STATUS,C
	incfsz	ACC2,W
	subwf	RE,W
	btfss	STATUS,C
	goto	SPI_CLOSE
	INCN	NEXTL, 2
	call	KV_COUNT
	btfss	STATUS,Z
	goto	SAMPLE_PICK
	goto	SPI_CLOSE		; (not reached: p sums to more than r)

; Opens LG_AREA for reading; IDX:ROWS = 512.
LG_OPEN:
	movlw	LOW (LG_AREA)
	movwf	SR0
	movlw	HIGH (LG_AREA)
	movwf	SR1
	movlw	UPPER (LG_AREA)
	movwf	SR2
	call	SR_OPEN_RD
	clrf	ROWS
	movlw	2
	movwf	IDX
	return

; W = p of the next logit: exp2_neg(l - max), 0 below MINP.
SAMPLE_P:
	call	SR_RD24
	SUBN	A0, MAXP, 4
	call	EXP2NEG
	movwf	T1
	movlw	MINP
	subwf	T1,W
	btfss	STATUS,C
	clrf	T1
	movf	T1,W
	return

; B0..B2 = A mod MINN (24-bit divisor).
MOD24:
	CLRN	B0, 4
	movlw	32
	movwf	CNT
MOD24_LOOP:
	ASLN	A0, 4
	rlf	B0,F
	rlf	B1,F
	rlf	B2,F
	rlf	B3,F
	movf	B3,F
	btfss	STATUS,Z
	goto	MOD24_SUB
	movf	MINN+2,W
	subwf	B2,W
	btfss	STATUS,C
	goto	MOD24_NEXT
	btfss	STATUS,Z
	goto	MOD24_SUB
	movf	MINN+1,W
	subwf	B1,W
	btfss	STATUS,C
	goto	MOD24_NEXT
	btfss	STATUS,Z
	goto	MOD24_SUB
	movf	MINN,W
	subwf	B0,W
	btfss	STATUS,C
	goto	MOD24_NEXT
MOD24_SUB:
	SUBN	B0, MINN, 3
	btfss	STATUS,C
	decf	B3,F
MOD24_NEXT:
	decfsz	CNT,F
	goto	MOD24_LOOP
	return

