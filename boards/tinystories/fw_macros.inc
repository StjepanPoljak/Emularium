; Multi-byte arithmetic on little-endian values in bank 0 or common RAM.

; dst (n bytes) += src (n bytes); C = carry out.
ADDN	macro	dst, src, n
	local	i
	movf	src,W
	addwf	dst,F
i = 1
	while i < n
	movf	src+i,W
	btfsc	STATUS,C
	incfsz	src+i,W
	addwf	dst+i,F
i = i + 1
	endw
	endm

; dst (n bytes) -= src (n bytes); C = no borrow.
SUBN	macro	dst, src, n
	local	i
	movf	src,W
	subwf	dst,F
i = 1
	while i < n
	movf	src+i,W
	btfss	STATUS,C
	incfsz	src+i,W
	subwf	dst+i,F
i = i + 1
	endw
	endm

; r (n bytes) = -r
NEGN	macro	r, n
	local	i
i = 0
	while i < n
	comf	r+i,F
i = i + 1
	endw
	incf	r,F
i = 1
	while i < n
	btfsc	STATUS,Z
	incf	r+i,F
i = i + 1
	endw
	endm

; r (n bytes) += 1
INCN	macro	r, n
	local	i
	incf	r,F
i = 1
	while i < n
	btfsc	STATUS,Z
	incf	r+i,F
i = i + 1
	endw
	endm

; Arithmetic shift right by 1.
ASRN	macro	r, n
	local	i
	rlf	r+n-1,W
i = n - 1
	while i >= 0
	rrf	r+i,F
i = i - 1
	endw
	endm

; Logical shift right by 1.
LSRN	macro	r, n
	local	i
	bcf	STATUS,C
i = n - 1
	while i >= 0
	rrf	r+i,F
i = i - 1
	endw
	endm

; Shift left by 1.
ASLN	macro	r, n
	local	i
	bcf	STATUS,C
i = 0
	while i < n
	rlf	r+i,F
i = i + 1
	endw
	endm

; dst = src, n bytes.
MOVN	macro	dst, src, n
	local	i
i = 0
	while i < n
	movf	src+i,W
	movwf	dst+i
i = i + 1
	endw
	endm

CLRN	macro	r, n
	local	i
i = 0
	while i < n
	clrf	r+i
i = i + 1
	endw
	endm

; Calls page-0 routine label from page-1 code, which runs with PCLATH<3>
; set.
C0	macro	label
	bcf	PCLATH,3
	call	label
	bsf	PCLATH,3
	endm

; Calls FLASH_RD (page 0) and restores PCLATH's page bits for the code
; around it.
FRD	macro
	if	$ >= 0x800
	bcf	PCLATH,3
	endif
	call	FLASH_RD
	if	$ >= 0x800
	bsf	PCLATH,3
	else
	bcf	PCLATH,3
	endif
	bcf	PCLATH,4
	endm

; Reads the flash byte at label + reg (8-bit index) into W.
FREAD	macro	label, reg
	movlw	LOW (label)
	addwf	reg,W
	movwf	FAL
	movlw	HIGH (label)
	btfsc	STATUS,C
	addlw	1
	movwf	FAH
	FRD
	endm

; A breakpoint for the tests: a labelled NOP.
CHECK	macro	name
name:
	nop
	endm
