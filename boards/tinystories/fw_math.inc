; Arithmetic, lookup tables and array addressing. Registers: A (32-bit,
; A0..A3), B (64-bit, B0..B7), T0, T1, CNT, all in common RAM.

; Jumps to the RETLW at FAH:FAL, which returns to FLASH_RD's caller with
; the byte in W. The caller resets PCLATH's page bits (FREAD does).
FLASH_RD:
	movf	FAH,W
	movwf	PCLATH
	movf	FAL,W
	movwf	PCL

; ------------------------------------------------------------ sign extension

SEXT8:
	clrf	A1
	btfsc	A0,7
	decf	A1,F
SEXT16:
	clrf	A2
	btfsc	A1,7
	decf	A2,F
SEXT24:
	clrf	A3
	btfsc	A2,7
	decf	A3,F
	return

; ------------------------------------------------------------ rounding shift

; A = rshift(A, T0): T0 > 0 rounds half up (shift T0-1, add 1, shift 1),
; T0 < 0 shifts left. Clobbers T0.
SHRR_A:
	movf	T0,W
	btfsc	STATUS,Z
	return
	btfsc	T0,7
	goto	SHRR_A_LEFT
	decf	T0,F
SHRR_A_BYTES:
	movlw	8
	subwf	T0,W
	btfss	STATUS,C
	goto	SHRR_A_BITS
	movwf	T0
	MOVN	A0, A0+1, 3
	movlw	0
	btfsc	A3,7
	movlw	0xFF
	movwf	A3
	goto	SHRR_A_BYTES
SHRR_A_BITS:
	movf	T0,F
	btfsc	STATUS,Z
	goto	SHRR_A_ROUND
SHRR_A_LOOP:
	ASRN	A0, 4
	decfsz	T0,F
	goto	SHRR_A_LOOP
SHRR_A_ROUND:
	INCN	A0, 4
	ASRN	A0, 4
	return
SHRR_A_LEFT:
	ASLN	A0, 4
	incfsz	T0,F
	goto	SHRR_A_LEFT
	return

; A = low 32 bits of rshift(B, T0), T0 > 0, rounding half up. Clobbers B, T0.
SHRR_B:
	decf	T0,F
SHRR_B_BYTES:
	movlw	8
	subwf	T0,W
	btfss	STATUS,C
	goto	SHRR_B_BITS
	movwf	T0
	MOVN	B0, B0+1, 7
	movlw	0
	btfsc	B7,7
	movlw	0xFF
	movwf	B7
	goto	SHRR_B_BYTES
SHRR_B_BITS:
	movf	T0,F
	btfsc	STATUS,Z
	goto	SHRR_B_ROUND
SHRR_B_LOOP:
	ASRN	B0, 8
	decfsz	T0,F
	goto	SHRR_B_LOOP
SHRR_B_ROUND:
	INCN	B0, 8
	ASRN	B0, 8
	MOVN	A0, B0, 4
	return

; ------------------------------------------------------------ multiply

; B1:B0 = W * T0, unsigned.
UMUL8:
	clrf	B1
	movwf	B0
	movf	T0,W
	bcf	STATUS,C
	rrf	B0,F
	variable	v_k
v_k = 0
	while v_k < 8
	btfsc	STATUS,C
	addwf	B1,F
	rrf	B1,F
	rrf	B0,F
v_k = v_k + 1
	endw
	return

; B0..B3 = A0..A1 * B0..B1, unsigned. Clobbers CNT.
UMUL16:
	clrf	B2
	clrf	B3
	movlw	16
	movwf	CNT
	bcf	STATUS,C
	rrf	B1,F
	rrf	B0,F
UMUL16_LOOP:
	btfss	STATUS,C
	goto	UMUL16_SHIFT
	movf	A0,W
	addwf	B2,F
	movf	A1,W
	btfsc	STATUS,C
	incfsz	A1,W
	addwf	B3,F
UMUL16_SHIFT:
	rrf	B3,F
	rrf	B2,F
	rrf	B1,F
	rrf	B0,F
	decfsz	CNT,F
	goto	UMUL16_LOOP
	return

; B0..B3 = A0..A1 * B0..B1, signed. Clobbers A0..A1, CNT, T1.
SMUL16:
	movf	A1,W
	xorwf	B1,W
	movwf	T1
	btfss	A1,7
	goto	SMUL16_A
	NEGN	A0, 2
SMUL16_A:
	btfss	B1,7
	goto	SMUL16_B
	NEGN	B0, 2
SMUL16_B:
	call	UMUL16
	btfss	T1,7
	return
	NEGN	B0, 4
	return

; B0..B7 = A0..A3 * B0..B3, unsigned. Clobbers CNT.
UMUL32:
	CLRN	B4, 4
	movlw	32
	movwf	CNT
	bcf	STATUS,C
	rrf	B3,F
	rrf	B2,F
	rrf	B1,F
	rrf	B0,F
UMUL32_LOOP:
	btfss	STATUS,C
	goto	UMUL32_SHIFT
	ADDN	B4, A0, 4
UMUL32_SHIFT:
	rrf	B7,F
	rrf	B6,F
	rrf	B5,F
	rrf	B4,F
	rrf	B3,F
	rrf	B2,F
	rrf	B1,F
	rrf	B0,F
	decfsz	CNT,F
	goto	UMUL32_LOOP
	return

; B0..B7 = A0..A3 * B0..B3, signed. Clobbers A, CNT, T1.
SMUL32:
	movf	A3,W
	xorwf	B3,W
	movwf	T1
	btfss	A3,7
	goto	SMUL32_A
	NEGN	A0, 4
SMUL32_A:
	btfss	B3,7
	goto	SMUL32_B
	NEGN	B0, 4
SMUL32_B:
	call	UMUL32
	btfss	T1,7
	return
	NEGN	B0, 8
	return

; ------------------------------------------------------------ division

; A = floor((2^32 + S/2) / S), S = SSH:SSL > 0. Clobbers B, CNT.
DIV_R:
	movf	SSL,W
	movwf	B0
	movf	SSH,W
	movwf	B1
	LSRN	B0, 2
	clrf	B2
	clrf	B3
	movlw	1
	movwf	B4
	CLRN	B5, 3
	CLRN	A0, 4
	movlw	40
	movwf	CNT
DIV_R_LOOP:
	ASLN	B0, 8			; next dividend bit into the remainder B7:B6:B5
	movf	B7,F
	btfss	STATUS,Z
	goto	DIV_R_SUB
	movf	SSH,W
	subwf	B6,W
	btfss	STATUS,C
	goto	DIV_R_NO
	btfss	STATUS,Z
	goto	DIV_R_SUB
	movf	SSL,W
	subwf	B5,W
	btfss	STATUS,C
	goto	DIV_R_NO
DIV_R_SUB:
	movf	SSL,W
	subwf	B5,F
	movf	SSH,W
	btfss	STATUS,C
	incfsz	SSH,W
	subwf	B6,F
	btfss	STATUS,C
	decf	B7,F
	bsf	STATUS,C
	goto	DIV_R_Q
DIV_R_NO:
	bcf	STATUS,C
DIV_R_Q:
	rlf	A0,F
	rlf	A1,F
	rlf	A2,F
	rlf	A3,F
	decfsz	CNT,F
	goto	DIV_R_LOOP
	return

; ------------------------------------------------------------ block floating point
;
; bfp8(v, F) of intmodel.py, for any number of values: BFP_RESET, then
; BFP_TRACK on each value (in A), BFP_FINISH with F in W, then BFP_CONV on
; each value again. MAXP/MINN hold the largest and smallest value.

BFP_RESET:
	CLRN	MAXP, 8
	return

BFP_TRACK:
	btfsc	A3,7
	goto	BFP_TRACK_NEG
	movf	A3,W
	subwf	MAXP+3,W
	btfss	STATUS,C
	goto	BFP_SETP
	btfss	STATUS,Z
	return
	movf	A2,W
	subwf	MAXP+2,W
	btfss	STATUS,C
	goto	BFP_SETP
	btfss	STATUS,Z
	return
	movf	A1,W
	subwf	MAXP+1,W
	btfss	STATUS,C
	goto	BFP_SETP
	btfss	STATUS,Z
	return
	movf	A0,W
	subwf	MAXP,W
	btfsc	STATUS,C
	return
BFP_SETP:
	MOVN	MAXP, A0, 4
	return
BFP_TRACK_NEG:
	btfss	MINN+3,7
	goto	BFP_SETN		; MINN still 0
	movf	MINN+3,W
	subwf	A3,W
	btfss	STATUS,C
	goto	BFP_SETN
	btfss	STATUS,Z
	return
	movf	MINN+2,W
	subwf	A2,W
	btfss	STATUS,C
	goto	BFP_SETN
	btfss	STATUS,Z
	return
	movf	MINN+1,W
	subwf	A1,W
	btfss	STATUS,C
	goto	BFP_SETN
	btfss	STATUS,Z
	return
	movf	MINN,W
	subwf	A0,W
	btfsc	STATUS,C
	return
BFP_SETN:
	MOVN	MINN, A0, 4
	return

; W = F. Sets BFPS (the shift) and BFPE (the exponent, F - BFPS).
BFP_FINISH:
	movwf	BFPE
	MOVN	A0, MINN, 4
	NEGN	A0, 4			; A = -MINN >= 0
	movf	A3,W
	subwf	MAXP+3,W
	btfss	STATUS,C
	goto	BFP_HAVEM
	btfss	STATUS,Z
	goto	BFP_TAKEP
	movf	A2,W
	subwf	MAXP+2,W
	btfss	STATUS,C
	goto	BFP_HAVEM
	btfss	STATUS,Z
	goto	BFP_TAKEP
	movf	A1,W
	subwf	MAXP+1,W
	btfss	STATUS,C
	goto	BFP_HAVEM
	btfss	STATUS,Z
	goto	BFP_TAKEP
	movf	A0,W
	subwf	MAXP,W
	btfss	STATUS,C
	goto	BFP_HAVEM
BFP_TAKEP:
	MOVN	A0, MAXP, 4
BFP_HAVEM:				; A = max |v|
	movf	A0,W
	iorwf	A1,W
	iorwf	A2,W
	iorwf	A3,W
	btfss	STATUS,Z
	goto	BFP_NONZERO
	clrf	BFPS			; all zero: E = F
	return
BFP_NONZERO:
	movlw	32
	movwf	T0
BFP_BITLEN:
	btfsc	A3,7
	goto	BFP_GOTLEN
	ASLN	A0, 4
	decf	T0,F
	goto	BFP_BITLEN
BFP_GOTLEN:
	movlw	7
	subwf	T0,W
	movwf	BFPS			; s = bitlen - 7
	MOVN	A0, MAXP, 4
	movf	BFPS,W
	movwf	T0
	call	SHRR_A
	btfsc	A0,7			; rshift(max, s) = 128
	goto	BFP_OVER
	MOVN	A0, MINN, 4
	movf	BFPS,W
	movwf	T0
	call	SHRR_A
	movf	A0,W
	xorlw	0x80			; rshift(min, s) = -128
	btfss	STATUS,Z
	goto	BFP_EXP
BFP_OVER:
	incf	BFPS,F
BFP_EXP:
	movf	BFPS,W
	subwf	BFPE,F
	return

; W = the mantissa of A.
BFP_CONV:
	movf	BFPS,W
	movwf	T0
	call	SHRR_A
	movf	A0,W
	return

; ------------------------------------------------------------ tables

; A1:A0 = sin of the phase in A1:A0 (10 bits, one turn), Q14 signed.
; Clobbers T0, T1.
SINF:
	movf	A1,W
	movwf	T1
	movf	A0,W
	btfsc	T1,0			; quadrants 1 and 3 run backwards
	xorlw	0xFF
	movwf	T0
	FREAD	SIN_LO, T0
	movwf	A0
	FREAD	SIN_HI, T0
	movwf	A1
	btfss	T1,1			; quadrants 2 and 3 are negative
	return
	NEGN	A0, 2
	return

; cos = sin(phase + quarter turn)
COSF:
	incf	A1,F
	movlw	0x03
	andwf	A1,F
	goto	SINF

; W = exp2_neg(A): 255 * 2^(A / 256) for A <= 0 (32-bit), as intmodel's
; Tables.exp2_neg. Clobbers A, T0.
EXP2NEG:
	NEGN	A0, 4
	movf	A3,W
	iorwf	A2,W
	btfss	STATUS,Z
	retlw	0
	movlw	16
	subwf	A1,W
	btfsc	STATUS,C
	retlw	0
	rrf	A0,W
	movwf	T0
	rrf	T0,F
	rrf	T0,W
	andlw	0x1F
	movwf	T0
	FREAD	EXP2, T0
	movwf	T0
	movf	A1,F
	btfsc	STATUS,Z
	goto	EXP2NEG_DONE
EXP2NEG_SHIFT:
	bcf	STATUS,C
	rrf	T0,F
	decfsz	A1,F
	goto	EXP2NEG_SHIFT
EXP2NEG_DONE:
	movf	T0,W
	return

; ------------------------------------------------------------ arrays

; FSR and IRP for x[W] (int16): elements 0..15 at 0x20 (bank 0), 16..63 at
; 0x190 (bank 3).
XPTR:
	movwf	FSR
	addwf	FSR,F
	movlw	32
	subwf	FSR,W
	btfss	STATUS,C
	goto	XPTR_LO
	addlw	0x90
	movwf	FSR
	bsf	STATUS,IRP
	return
XPTR_LO:
	movlw	0x20
	addwf	FSR,F
	bcf	STATUS,IRP
	return

; A = x[W], sign-extended.
XLOAD:
	call	XPTR
	movf	INDF,W
	movwf	A0
	incf	FSR,F
	movf	INDF,W
	movwf	A1
	goto	SEXT16

; x[W] = A1:A0
XSTORE:
	call	XPTR
	movf	A0,W
	movwf	INDF
	incf	FSR,F
	movf	A1,W
	movwf	INDF
	return

; FSR, IRP at the first activation: 0..95 in bank 2 (0x110), 96..171 in
; bank 1 (0xA0).
ACT_FIRST:
	bsf	STATUS,IRP
	movlw	0x10
	movwf	FSR
	return

ACT_NEXT:
	incf	FSR,F
	movf	FSR,W
	xorlw	0x70
	btfss	STATUS,Z
	return
	movlw	0xA0
	movwf	FSR
	bcf	STATUS,IRP
	return
