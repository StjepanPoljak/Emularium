; TinyStories on one PIC16F876: Karpathy's stories260K language model,
; run token by token with the integer arithmetic of tinystories/intmodel.py
; (bit-exact), writing the story on the LCD.
;
; Weights: three 25LC1024 EEPROMs holding tinystories/out/weights_n1_k0.bin
; (format in tinystories/export.py), streamed once per token. KV cache and
; scratch: a 23LC1024 SPI SRAM. Tables, vocabulary and font: data.inc
; (gen_data.py).
;
; Build: gpasm -p p16f876 -o tinystories.hex tinystories.asm
;        (-D SEED=n for another random seed; default 1)

	LIST P=16F876, R=DEC
	#include <p16f876.inc>
	__CONFIG _HS_OSC & _WDT_OFF & _PWRTE_ON & _BODEN_OFF & _LVP_OFF & _CP_OFF
	ERRORLEVEL -302, -1302

#ifndef SEED
#define SEED 1
#endif

; ------------------------------------------------------------ pins

#define EE0_CS	PORTC,0
#define EE1_CS	PORTC,1
#define RAM_CS	PORTC,2
#define SCK	PORTC,3
#define MISO	PORTC,4
#define MOSI	PORTC,5
#define EE2_CS	PORTA,5
#define LCD_RS	PORTA,0
#define LCD_RW	PORTA,1
#define LCD_E	PORTA,2

; ------------------------------------------------------------ model and memory map

DIM		EQU	64
HIDDEN		EQU	172
NLAYERS		EQU	5
MINP		EQU	8
LAYER0_AT	EQU	16		; image header length
EMB_AT		EQU	235576		; 16 + 5 layers of 47112 bytes

SR_READ		EQU	0x03
SR_WRITE	EQU	0x02

; SRAM: KV cache from 0 ((layer * 4 + kv head) * 256 + slot) * 18 bytes,
; [k 8][k exp][v 8][v exp]; then scratch.
O_AREA		EQU	0x18000		; attention output, 64 x 24 bit
HB_AREA		EQU	0x18100		; FFN hidden vector, 172 x 24 bit
LG_AREA		EQU	0x18400		; logits, 512 x 24 bit
TXT_AREA	EQU	0x18A00		; 10 text lines of 32 characters
TXT_STATE	EQU	0x18B40		; top line, current line, column

; Common RAM (any bank)
SPIB	EQU	0x70
CNT	EQU	0x71
T0	EQU	0x72
T1	EQU	0x73
A0	EQU	0x74			; A: 32 bit
A1	EQU	0x75
A2	EQU	0x76
A3	EQU	0x77
B0	EQU	0x78			; B: 64 bit
B1	EQU	0x79
B2	EQU	0x7A
B3	EQU	0x7B
B4	EQU	0x7C
B5	EQU	0x7D
B6	EQU	0x7E
B7	EQU	0x7F

; Bank 0 (0x20..0x3F hold x[0..15])
ACC0	EQU	0x40			; matmul accumulator (ACC0 is also PRODL)
ACC1	EQU	0x41
ACC2	EQU	0x42
PRODH	EQU	0x43
SL	EQU	0x44			; row header: sum of weights, exponent
SH	EQU	0x45
RE	EQU	0x46
SUMAL	EQU	0x47			; A = sum of the offset activations
SUMAH	EQU	0x48
EV	EQU	0x49			; activation exponent
NIN	EQU	0x4A			; inputs per row (64 or 172)
ROWS	EQU	0x4B
MMH	EQU	0x4C			; row handler: 0 store, 1 residual, 2 logit
IDX	EQU	0x4D			; row index
OPTR	EQU	0x4E			; store handler's FSR
FLAGS	EQU	0x4F
SA0	EQU	0x50			; weight stream address
SA1	EQU	0x51
SA2	EQU	0x52
SR0	EQU	0x53			; SRAM address
SR1	EQU	0x54
SR2	EQU	0x55
FAL	EQU	0x56			; flash address for FLASH_RD
FAH	EQU	0x57
LAYER	EQU	0x58
GRP	EQU	0x59			; KV head; FFN unit index
HQ	EQU	0x5A			; query head within the KV head
BFPS	EQU	0x5B
BFPE	EQU	0x5C
MAXP	EQU	0x5D			; 4 bytes; 8 bytes with MINN: PSUM, SS
MINN	EQU	0x61			; 4 bytes
POSL	EQU	0x65
POSH	EQU	0x66
TOKL	EQU	0x67			; input token
TOKH	EQU	0x68
NCTXL	EQU	0x69			; context length; next token after SAMPLE
NCTXH	EQU	0x6A
E0	EQU	0x6B
QE	EQU	0x6C
KE	EQU	0x6D
VE	EQU	0x6E
XT	EQU	0x6F			; scratch: shift, QA base, phase index

; FLAGS
EEOPEN	EQU	0			; weight stream open at SA
OIRP	EQU	1			; store handler writes to banks 2/3
RAWACT	EQU	2			; RMSNORM leaves signed mantissas, no A

; Aliases by phase
SS	EQU	MAXP			; RMSNORM: 40-bit sum of squares
SMAX	EQU	MAXP			; attention: score max; then R
SSL	EQU	MINN			; attention: sum of weights
SSH	EQU	MINN+1
PWL	EQU	MINN+2			; attention: weight << 6 >> (vE - E0)
PWH	EQU	MINN+3
PSUM	EQU	MAXP			; RoPE: 64-bit product sum
RTA	EQU	SL			; RoPE: a (3 bytes)
RTB	EQU	E0			; RoPE: b (3 bytes)
RP_PTR	EQU	NCTXL
RP_J	EQU	NCTXH
NEXTL	EQU	NCTXL
NEXTH	EQU	NCTXH
; Printing (between tokens)
P_D	EQU	ACC0
P_D2	EQU	ACC1
P_D3	EQU	ACC2
P_Y	EQU	PRODH
P_W	EQU	SL
P_HI	EQU	SH
P_LO	EQU	RE
P_R	EQU	SUMAL
P_K	EQU	SUMAH
P_L	EQU	EV
P_CH	EQU	NIN
P_TOP	EQU	ROWS
P_CUR	EQU	MMH
P_COL	EQU	IDX
P_LEN	EQU	OPTR
P_OFSL	EQU	LAYER
P_OFSH	EQU	GRP
P_IL	EQU	HQ
P_IH	EQU	BFPS
P_K2	EQU	E0			; word wrap: last space, tail length, index
P_N	EQU	QE
P_I2	EQU	KE
P_PEND	EQU	VE			; the character PUTC is printing

; Working arrays (FSR; IRP 0 = banks 0/1, 1 = banks 2/3)
WQ	EQU	0xA0			; q, 16 x 24 bit (bank 1)
WK	EQU	0xD0			; k, 8 x 24 bit (bank 1)
WV	EQU	0x50			; v, 8 x 24 bit (bank 2, IRP 1)
PH	EQU	0x68			; RoPE phases, 4 x 16 bit (bank 2, IRP 1)
AACC	EQU	0x50			; attention accumulators, 8 x 32 bit (bank 2)
VABUF	EQU	0xD0			; v mantissas of one cache entry (bank 1)
H1	EQU	0xA0			; FFN (bank 1)
H3	EQU	0xA3
RNG	EQU	0xEC			; xorshift32 state (bank 1)
TXTBUF	EQU	0x110			; a text line (bank 2)
TAILBUF	EQU	0x130			; a word moving to the next line (bank 2)

	#include "fw_macros.inc"

; Program memory: page 0 (0x000) holds everything the inner loops call;
; page 1 (0x800) the rest, which calls page 0 with C0 and runs with
; PCLATH<3> set; data from 0x1000 is read with FLASH_RD.
	ORG	0
	bsf	PCLATH,3
	goto	START

	#include "fw_spi.inc"
	#include "fw_math.inc"
	#include "fw_core.inc"
PAGE0_END:

	ORG	0x800


; ------------------------------------------------------------ start

START:
	clrf	STATUS
	clrf	PCLATH
	clrf	FLAGS
	movlw	B'00100000'		; EE2_CS high; LCD RS, RW, E low
	movwf	PORTA
	clrf	PORTB
	movlw	B'00000111'		; EE0_CS, EE1_CS, RAM_CS high, SCK low
	movwf	PORTC
	bsf	STATUS,RP0
	movlw	0x06			; PORTA digital
	movwf	ADCON1
	movlw	B'11011000'		; RA0-RA2 (LCD control), RA5 outputs
	movwf	TRISA
	clrf	TRISB			; LCD data
	movlw	B'11010000'		; RC0-RC3, RC5 outputs; RC4 (MISO) input
	movwf	TRISC
	bcf	STATUS,RP0
	call	LCD_INIT
	call	TXT_INIT
	C0	RNG_INIT

NEW_STORY:
	clrf	POSL
	clrf	POSH
	movlw	1			; BOS
	movwf	TOKL
	clrf	TOKH
TOKEN_LOOP:
	call	FORWARD
	C0	SAMPLE
	CHECK	CK_TOKEN
	movf	NEXTH,F
	btfss	STATUS,Z
	goto	PRINT_NEXT
	decf	NEXTL,W
	btfss	STATUS,Z
	goto	PRINT_NEXT
	call	STORY_END		; BOS: the story is over
	goto	NEW_STORY
PRINT_NEXT:
	call	PRINT_TOKEN
	MOVN	TOKL, NEXTL, 2
	INCN	POSL, 2
	goto	TOKEN_LOOP

; A blank line after a story.
STORY_END:
	call	TXT_LOAD
	movf	P_COL,F
	btfss	STATUS,Z
	call	NEWLINE
	call	NEWLINE
	goto	TXT_SAVE

; ------------------------------------------------------------ forward pass

; Logits for token TOKL/TOKH at position POSL/POSH into LG_AREA, their
; maximum in MAXP.
FORWARD:
	; x = the token's embedding row: rshift(w, e - 10)
	MOVN	A0, TOKL, 2
	movlw	67
	movwf	B0
	clrf	B1
	C0	UMUL16
	movlw	LOW (EMB_AT)
	movwf	SA0
	movlw	HIGH (EMB_AT)
	movwf	SA1
	movlw	UPPER (EMB_AT)
	movwf	SA2
	ADDN	SA0, B0, 3
	C0	SPI_CLOSE
	C0	EE_RD
	C0	EE_RD
	C0	EE_RD
	movwf	RE
	clrf	IDX
FWD_EMB:
	C0	EE_RD
	xorlw	0x80
	movwf	A0
	C0	SEXT8
	movlw	10
	subwf	RE,W
	movwf	T0
	C0	SHRR_A
	movf	IDX,W
	C0	XSTORE
	incf	IDX,F
	btfss	IDX,6
	goto	FWD_EMB
	CHECK	CK_EMB

	movlw	LAYER0_AT		; the layers, in one stream
	movwf	SA0
	clrf	SA1
	clrf	SA2
	C0	SPI_CLOSE
	clrf	LAYER
FWD_LAYER:
	bcf	FLAGS,RAWACT
	call	RMSNORM
	CHECK	CK_ATTNORM
	clrf	GRP
FWD_GROUP:
	call	QKV
	CHECK	CK_QKV
	C0	KV_WRITE
	CHECK	CK_KV
	clrf	HQ
	C0	ATTEND
	incf	HQ,F
	C0	ATTEND
	incf	GRP,F
	btfss	GRP,2
	goto	FWD_GROUP
	CHECK	CK_ATTOUT
	movlw	LOW (O_AREA)
	movwf	SR0
	movlw	HIGH (O_AREA)
	movwf	SR1
	movlw	UPPER (O_AREA)
	movwf	SR2
	movlw	DIM
	movwf	NIN
	C0	BFP_SRAM
	movlw	1			; wo, added to x
	movwf	MMH
	movlw	DIM
	movwf	ROWS
	C0	MM
	CHECK	CK_XATT
	call	RMSNORM
	CHECK	CK_FFNNORM
	call	FFN_HIDDEN
	CHECK	CK_HB
	movlw	LOW (HB_AREA)
	movwf	SR0
	movlw	HIGH (HB_AREA)
	movwf	SR1
	movlw	UPPER (HB_AREA)
	movwf	SR2
	movlw	HIDDEN
	movwf	NIN
	C0	BFP_SRAM
	movlw	1			; w2, added to x
	movwf	MMH
	movlw	DIM
	movwf	ROWS
	C0	MM
	CHECK	CK_XFFN
	incf	LAYER,F
	movf	LAYER,W
	xorlw	NLAYERS
	btfss	STATUS,Z
	goto	FWD_LAYER

	; final RMSNorm with its gain, then the classifier (the embedding rows)
	bsf	FLAGS,RAWACT
	call	RMSNORM
	call	GAIN_NORM
	movlw	DIM
	movwf	NIN
	movlw	2
	movwf	MMH
	movlw	LOW (LG_AREA)
	movwf	SR0
	movlw	HIGH (LG_AREA)
	movwf	SR1
	movlw	UPPER (LG_AREA)
	movwf	SR2
	CLRN	MAXP, 3			; max = -2^24
	movlw	0xFF
	movwf	MAXP+3
	clrf	ROWS			; 2 x 256 rows
	C0	MM
	clrf	ROWS
	C0	MM
	C0	SPI_CLOSE
	CHECK	CK_LOGITS
	return

; ------------------------------------------------------------ RMSNorm

; ACT, SUMA, EV = the BFP activations of x / rms(x) (rmsnorm_bfp without
; gain). With FLAGS.RAWACT, ACT gets the signed mantissas and BFPE the
; exponent, for GAIN_NORM.
RMSNORM:
	CLRN	SS, 5
	clrf	IDX
RMS_SQ:
	movf	IDX,W
	C0	XLOAD
	btfss	A1,7
	goto	RMS_SQ_POS
	NEGN	A0, 2
RMS_SQ_POS:
	MOVN	B0, A0, 2
	C0	UMUL16
	ADDN	SS, B0, 4
	btfsc	STATUS,C
	incf	SS+4,F
	incf	IDX,F
	btfss	IDX,6
	goto	RMS_SQ
	; ms = (ss >> 6) + 10
	variable	v_r
v_r = 0
	while v_r < 6
	LSRN	SS, 5
v_r = v_r + 1
	endw
	MOVN	A0, SS, 4
	movlw	10
	addwf	A0,F
	btfss	STATUS,C
	goto	RMS_MS
	INCN	A1, 3
RMS_MS:
	; s = bitlen(ms) - 16, rounded up to even
	MOVN	B0, A0, 4
	movlw	32
	movwf	T1
RMS_BITLEN:
	btfsc	B3,7
	goto	RMS_GOTLEN
	ASLN	B0, 4
	decf	T1,F
	goto	RMS_BITLEN
RMS_GOTLEN:
	movlw	16
	subwf	T1,W
	movwf	XT
	btfsc	XT,0
	incf	XT,F
	; n' = ms >> s (or << -s); index n' >> 8
	movf	XT,W
	btfsc	STATUS,Z
	goto	RMS_IDX
	movwf	T1
	btfsc	XT,7
	goto	RMS_LEFT
RMS_RIGHT:
	LSRN	A0, 4
	decfsz	T1,F
	goto	RMS_RIGHT
	goto	RMS_IDX
RMS_LEFT:
	ASLN	A0, 4
	incfsz	T1,F
	goto	RMS_LEFT
RMS_IDX:
	movf	A1,W
	addlw	-64
	movwf	T0
	FREAD	RSQRT_LO, T0
	movwf	SL
	FREAD	RSQRT_HI, T0
	movwf	SH
	; e = 12 + s / 2
	rlf	XT,W
	rrf	XT,W
	addlw	12
	movwf	RE
	; bfp8(x * m, 10 + e)
	C0	BFP_RESET
	clrf	IDX
RMS_TRACK:
	C0	XM
	C0	BFP_TRACK
	incf	IDX,F
	btfss	IDX,6
	goto	RMS_TRACK
	movf	RE,W
	addlw	10
	C0	BFP_FINISH
	clrf	SUMAL
	clrf	SUMAH
	clrf	IDX
RMS_CONV:
	C0	XM
	C0	BFP_CONV
	btfss	FLAGS,RAWACT
	xorlw	0x80
	C0	ACT_PUT
	incf	IDX,F
	btfss	IDX,6
	goto	RMS_CONV
	movf	BFPE,W
	movwf	EV
	return

; The final norm's gain: ACT holds the signed mantissas a with exponent
; BFPE; ACT, SUMA, EV = bfp8(a * gain, E + 12).
GAIN_NORM:
	movf	BFPE,W
	movwf	XT
	C0	BFP_RESET
	clrf	IDX
GAIN_TRACK:
	call	GAIN_MUL
	C0	BFP_TRACK
	incf	IDX,F
	btfss	IDX,6
	goto	GAIN_TRACK
	movf	XT,W
	addlw	12
	C0	BFP_FINISH
	clrf	SUMAL
	clrf	SUMAH
	clrf	IDX
GAIN_CONV:
	call	GAIN_MUL
	C0	BFP_CONV
	xorlw	0x80
	C0	ACT_PUT
	incf	IDX,F
	btfss	IDX,6
	goto	GAIN_CONV
	movf	BFPE,W
	movwf	EV
	return

; A = ACT[IDX] (signed; offset if already converted) * FINAL_GAIN[IDX].
; Conversion happens in place, so the second pass reads offset values:
; GAIN_CONV runs over IDX in order and only rewrites ACT[IDX] after
; reading it.
GAIN_MUL:
	movf	IDX,W
	C0	ACT_PTR
	movf	INDF,W
	movwf	A0
	C0	SEXT8
	bcf	STATUS,C
	rlf	IDX,W
	movwf	T0
	FREAD	FINAL_GAIN, T0
	movwf	B0
	incf	T0,F
	FREAD	FINAL_GAIN, T0
	movwf	B1
	C0	SMUL16
	MOVN	A0, B0, 4
	return

; ------------------------------------------------------------ q, k, v, RoPE

QKV:
	clrf	MMH
	movlw	DIM
	movwf	NIN
	bcf	FLAGS,OIRP
	movlw	WQ
	movwf	OPTR
	movlw	16
	movwf	ROWS
	C0	MM
	movlw	WK
	movwf	OPTR
	movlw	8
	movwf	ROWS
	C0	MM
	bsf	FLAGS,OIRP
	movlw	WV
	movwf	OPTR
	movlw	8
	movwf	ROWS
	C0	MM
	call	PHASES
	clrf	XT			; q: 8 pairs, 2 heads x 4
QKV_ROPE_Q:
	movf	XT,W			; RP_PTR = WQ + 6 XT
	addwf	XT,W
	movwf	RP_PTR
	addwf	RP_PTR,F
	addwf	RP_PTR,F
	movlw	WQ
	addwf	RP_PTR,F
	movf	XT,W
	andlw	3
	movwf	RP_J
	call	ROPE_PAIR
	incf	XT,F
	btfss	XT,3
	goto	QKV_ROPE_Q
	clrf	XT			; k: 4 pairs
QKV_ROPE_K:
	movf	XT,W
	addwf	XT,W
	movwf	RP_PTR
	addwf	RP_PTR,F
	addwf	RP_PTR,F
	movlw	WK
	addwf	RP_PTR,F
	movf	XT,W
	movwf	RP_J
	call	ROPE_PAIR
	incf	XT,F
	btfss	XT,2
	goto	QKV_ROPE_K
	return

; PH[j] = ((pos * ROPE_INC_j) >> 14) & 1023: the phase of pair j, in
; 1/1024 turns.
PHASE	macro	j, inc
	movlw	LOW (inc)
	movwf	A0
	movlw	HIGH (inc)
	movwf	A1
	movlw	UPPER (inc)
	movwf	A2
	clrf	A3
	MOVN	B0, POSL, 2
	clrf	B2
	clrf	B3
	C0	UMUL32
	variable	v_q
v_q = 0
	while v_q < 6
	LSRN	B1, 2
v_q = v_q + 1
	endw
	bsf	STATUS,IRP
	movlw	PH + 2 * j
	movwf	FSR
	movf	B1,W
	movwf	INDF
	incf	FSR,F
	movf	B2,W
	andlw	0x03
	movwf	INDF
	endm

PHASES:
	PHASE	0, ROPE_INC0
	PHASE	1, ROPE_INC1
	PHASE	2, ROPE_INC2
	PHASE	3, ROPE_INC3
	return

; B0..B3 = sin or cos of phase RP_J, sign-extended.
LOAD_PH:
	bsf	STATUS,IRP
	bcf	STATUS,C
	rlf	RP_J,W
	addlw	PH
	movwf	FSR
	movf	INDF,W
	movwf	A0
	incf	FSR,F
	movf	INDF,W
	movwf	A1
	return
LOAD_SIN:
	call	LOAD_PH
	C0	SINF
	goto	A16_TO_B
LOAD_COS:
	call	LOAD_PH
	C0	COSF
A16_TO_B:
	C0	SEXT16
	MOVN	B0, A0, 4
	return

; A = the 24-bit value at FSR (bank 1), sign-extended; FSR moves past it.
LOAD24:
	bcf	STATUS,IRP
	movf	INDF,W
	movwf	A0
	incf	FSR,F
	movf	INDF,W
	movwf	A1
	incf	FSR,F
	movf	INDF,W
	movwf	A2
	incf	FSR,F
	C0	SEXT24
	return

; Writes A0..A2 at FSR (bank 1).
STORE24:
	bcf	STATUS,IRP
	movf	A0,W
	movwf	INDF
	incf	FSR,F
	movf	A1,W
	movwf	INDF
	incf	FSR,F
	movf	A2,W
	movwf	INDF
	return

; Rotates the pair (a, b) at RP_PTR (bank 1) by phase RP_J:
; a' = rshift(a cos - b sin, 14), b' = rshift(a sin + b cos, 14).
ROPE_PAIR:
	movf	RP_PTR,W
	movwf	FSR
	call	LOAD24
	MOVN	RTA, A0, 3
	call	LOAD24
	MOVN	RTB, A0, 3
	call	LOAD_COS		; PSUM = a cos
	call	ROPE_A
	C0	SMUL32
	MOVN	PSUM, B0, 8
	call	LOAD_SIN		; PSUM -= b sin
	call	ROPE_B
	C0	SMUL32
	SUBN	PSUM, B0, 8
	MOVN	B0, PSUM, 8
	movlw	14
	movwf	T0
	C0	SHRR_B
	movf	RP_PTR,W
	movwf	FSR
	call	STORE24
	call	LOAD_SIN		; PSUM = a sin
	call	ROPE_A
	C0	SMUL32
	MOVN	PSUM, B0, 8
	call	LOAD_COS		; PSUM += b cos
	call	ROPE_B
	C0	SMUL32
	ADDN	PSUM, B0, 8
	MOVN	B0, PSUM, 8
	movlw	14
	movwf	T0
	C0	SHRR_B
	movf	RP_PTR,W
	addlw	3
	movwf	FSR
	goto	STORE24

ROPE_A:
	MOVN	A0, RTA, 3
	C0	SEXT24
	return
ROPE_B:
	MOVN	A0, RTB, 3
	C0	SEXT24
	return

; ------------------------------------------------------------ FFN

; hb[i] = rshift(rshift(h1 sigmoid(h1), 8) h3, 12) for the 172 hidden
; units, to HB_AREA.
FFN_HIDDEN:
	movlw	LOW (HB_AREA)
	movwf	SR0
	movlw	HIGH (HB_AREA)
	movwf	SR1
	movlw	UPPER (HB_AREA)
	movwf	SR2
	clrf	GRP
FFN_LOOP:
	clrf	MMH
	movlw	DIM
	movwf	NIN
	bcf	FLAGS,OIRP
	movlw	H1
	movwf	OPTR
	movlw	1
	movwf	ROWS
	C0	MM
	movlw	H3
	movwf	OPTR
	movlw	1
	movwf	ROWS
	C0	MM
	; sigmoid(h1) in Q8: B0..B3
	movlw	H1
	movwf	FSR
	call	LOAD24
	MOVN	B0, A0, 4
	btfss	B3,7
	goto	FFN_ABS
	NEGN	B0, 4
FFN_ABS:
	movf	B3,W			; |h1| >> 9 >= 64: 256
	iorwf	B2,W
	btfss	STATUS,Z
	goto	FFN_SIG_ONE
	btfsc	B1,7
	goto	FFN_SIG_ONE
	rrf	B1,W
	andlw	0x3F
	movwf	T0
	FREAD	SIGMOID, T0
	clrf	B1
	addlw	128			; the table holds s - 128; s can be 256
	movwf	B0
	btfsc	STATUS,C
	incf	B1,F
	goto	FFN_SIG_SIGN
FFN_SIG_ONE:
	clrf	B0
	movlw	1
	movwf	B1
FFN_SIG_SIGN:
	clrf	B2
	clrf	B3
	btfss	A3,7			; h1 < 0: 256 - s
	goto	FFN_SILU
	comf	B0,F
	comf	B1,F
	INCN	B0, 2
	movlw	1
	addwf	B1,F
FFN_SILU:
	C0	SMUL32		; silu = rshift(h1 s, 8)
	movlw	8
	movwf	T0
	C0	SHRR_B
	movlw	H3			; hb = rshift(silu h3, 12)
	movwf	FSR
	bcf	STATUS,IRP
	movf	INDF,W
	movwf	B0
	incf	FSR,F
	movf	INDF,W
	movwf	B1
	incf	FSR,F
	movf	INDF,W
	movwf	B2
	clrf	B3
	btfsc	B2,7
	decf	B3,F
	C0	SMUL32
	movlw	12
	movwf	T0
	C0	SHRR_B
	C0	SR_OPEN_WR
	C0	SR_WR24
	C0	SPI_CLOSE
	C0	SR_ADD3
	incf	GRP,F
	movf	GRP,W
	xorlw	HIDDEN
	btfss	STATUS,Z
	goto	FFN_LOOP
	return


	#include "fw_lcd.inc"
PAGE1_END:

	ORG	0x1000
; ------------------------------------------------------------ data

	#include "data.inc"

	END
