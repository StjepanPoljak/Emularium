; ST7920 LCD in graphics mode, 8-bit parallel (RA0 RS, RA1 RW, RA2 E,
; PORTB data), with fixed delays instead of busy polling: 10 lines of 32
; characters in a 4x6 font. The text of the 10 lines is kept in SRAM
; (TXT_AREA, a ring of lines) so the screen can scroll.
;
; Print-time variables reuse RAM the forward pass doesn't need between
; tokens (see the P_ names in tinystories.asm).

; About 80 us at 20 MHz: an ST7920 instruction takes 72 us.
LCD_DELAY:
	movlw	130
	movwf	P_D
LCD_DELAY_LOOP:
	decfsz	P_D,F
	goto	LCD_DELAY_LOOP
	return

LCD_CMD:
	bcf	LCD_RS
	goto	LCD_OUT
LCD_DATA:
	bsf	LCD_RS
LCD_OUT:
	movwf	PORTB
	bcf	LCD_RW
	bsf	LCD_E
	nop
	bcf	LCD_E
	goto	LCD_DELAY

; Writes the 16 pixels P_HI:P_LO at screen row P_Y (0..63), word P_W
; (0..7). GDRAM rows 32..63 are the right half of rows 0..31.
LCD_WORD:
	movf	P_Y,W
	andlw	0x1F
	iorlw	0x80
	call	LCD_CMD			; vertical address
	movf	P_W,W
	btfsc	P_Y,5
	addlw	8
	iorlw	0x80
	call	LCD_CMD			; horizontal address
	movf	P_HI,W
	call	LCD_DATA
	movf	P_LO,W
	goto	LCD_DATA

; ~W ms (W > 0).
DELAY_MS:
	movwf	P_D2
DELAY_MS_LOOP:
	movlw	12
	movwf	P_D3
DELAY_MS_INNER:
	call	LCD_DELAY
	decfsz	P_D3,F
	goto	DELAY_MS_INNER
	decfsz	P_D2,F
	goto	DELAY_MS_LOOP
	return

LCD_INIT:
	movlw	50
	call	DELAY_MS
	movlw	0x30			; 8-bit interface, basic instructions
	call	LCD_CMD
	movlw	0x30
	call	LCD_CMD
	movlw	0x0C			; display on
	call	LCD_CMD
	movlw	0x01			; clear
	call	LCD_CMD
	movlw	2
	call	DELAY_MS
	movlw	0x06			; entry mode
	call	LCD_CMD
	movlw	0x34			; extended instructions
	call	LCD_CMD
	movlw	0x36			; graphics on
	call	LCD_CMD
	clrf	P_HI
	clrf	P_LO
	clrf	P_Y
LCD_CLEAR_ROW:
	clrf	P_W
LCD_CLEAR_WORD:
	call	LCD_WORD
	incf	P_W,F
	btfss	P_W,3
	goto	LCD_CLEAR_WORD
	incf	P_Y,F
	btfss	P_Y,6
	goto	LCD_CLEAR_ROW
	return

; W = row P_R (0..5) of the glyph for character W, in the low nibble.
GLYPH:
	addlw	-32
	movwf	T0
	sublw	94			; printable: 0..94
	btfss	STATUS,C
	retlw	0
	movlw	LOW (FONT)
	movwf	FAL
	movlw	HIGH (FONT)
	movwf	FAH
	movf	T0,W			; + 3 * (char - 32)
	addwf	FAL,F
	btfsc	STATUS,C
	incf	FAH,F
	addwf	FAL,F
	btfsc	STATUS,C
	incf	FAH,F
	addwf	FAL,F
	btfsc	STATUS,C
	incf	FAH,F
	bcf	STATUS,C		; + row / 2
	rrf	P_R,W
	addwf	FAL,F
	btfsc	STATUS,C
	incf	FAH,F
	FRD
	movwf	T0
	btfss	P_R,0			; even rows are the high nibble
	swapf	T0,F
	movf	T0,W
	andlw	0x0F
	return

; Draws word P_W (characters 4 P_W .. 4 P_W + 3 of TXTBUF) on text line
; P_K of the screen.
RENDER_WORD:
	clrf	P_R
RENDER_WORD_ROW:
	movf	P_K,W			; P_Y = 6 P_K + P_R
	movwf	P_Y
	addwf	P_Y,F
	addwf	P_Y,F
	movf	P_Y,W
	addwf	P_Y,F
	movf	P_R,W
	addwf	P_Y,F
	movf	P_W,W			; FSR = TXTBUF + 4 P_W
	movwf	FSR
	rlf	FSR,F
	rlf	FSR,W
	andlw	0x1C
	addlw	LOW (TXTBUF)
	movwf	FSR
	bsf	STATUS,IRP
	movf	INDF,W
	call	GLYPH
	movwf	P_HI
	swapf	P_HI,F
	incf	FSR,F
	movf	INDF,W
	call	GLYPH
	iorwf	P_HI,F
	incf	FSR,F
	movf	INDF,W
	call	GLYPH
	movwf	P_LO
	swapf	P_LO,F
	incf	FSR,F
	movf	INDF,W
	call	GLYPH
	iorwf	P_LO,F
	call	LCD_WORD
	incf	P_R,F
	movf	P_R,W
	xorlw	6
	btfss	STATUS,Z
	goto	RENDER_WORD_ROW
	return

; SR = TXT_AREA + 32 W (text line W).
TXT_ADDR:
	movwf	SR0
	clrf	SR1			; W >> 3 for W < 10
	btfsc	SR0,3
	incf	SR1,F
	movlw	HIGH (TXT_AREA)
	addwf	SR1,F
	movlw	UPPER (TXT_AREA)
	movwf	SR2
	swapf	SR0,F			; (W & 7) << 5
	rlf	SR0,F
	movlw	0xE0
	andwf	SR0,F
	return

; TXTBUF = text line W.
LOAD_LINE:
	call	TXT_ADDR
	C0	SR_OPEN_RD
	bsf	STATUS,IRP
	movlw	LOW (TXTBUF)
	movwf	FSR
LOAD_LINE_LOOP:
	C0	SPI_RD
	movwf	INDF
	incf	FSR,F
	movf	FSR,W
	xorlw	LOW (TXTBUF) + 32
	btfss	STATUS,Z
	goto	LOAD_LINE_LOOP
	C0	SPI_CLOSE
	return

; Fills text line W with spaces.
CLEAR_LINE:
	call	TXT_ADDR
	C0	SR_OPEN_WR
	movlw	32
	movwf	P_D2
CLEAR_LINE_LOOP:
	movlw	' '
	C0	SPI_WR
	decfsz	P_D2,F
	goto	CLEAR_LINE_LOOP
	C0	SPI_CLOSE
	return

; Redraws all 10 lines, oldest (P_TOP) first.
REDRAW:
	clrf	P_K
REDRAW_LINE:
	movf	P_K,W			; line (P_TOP + P_K) mod 10
	addwf	P_TOP,W
	movwf	P_L
	movlw	10
	subwf	P_L,W
	btfsc	STATUS,C
	movwf	P_L
	movf	P_L,W
	call	LOAD_LINE
	clrf	P_W
REDRAW_WORD:
	call	RENDER_WORD
	incf	P_W,F
	btfss	P_W,3
	goto	REDRAW_WORD
	incf	P_K,F
	movf	P_K,W
	xorlw	10
	btfss	STATUS,Z
	goto	REDRAW_LINE
	return

NEWLINE:
	clrf	P_COL
	incf	P_CUR,F
	movf	P_CUR,W
	xorlw	10
	btfsc	STATUS,Z
	clrf	P_CUR
	movf	P_CUR,W
	call	CLEAR_LINE
	movf	P_CUR,W
	xorwf	P_TOP,W
	btfss	STATUS,Z
	return				; the screen isn't full yet
	incf	P_TOP,F			; scroll up a line
	movf	P_TOP,W
	xorlw	10
	btfsc	STATUS,Z
	clrf	P_TOP
	goto	REDRAW

; Prints character W. A space that doesn't fit is dropped; a word that
; doesn't fit moves to the next line (WRAP_WORD).
PUTC:
	movwf	P_PEND
	xorlw	10
	btfsc	STATUS,Z
	goto	NEWLINE
	movlw	32
	subwf	P_COL,W
	btfss	STATUS,C
	goto	PUTC_PEND
	movf	P_PEND,W
	xorlw	' '
	btfsc	STATUS,Z
	goto	NEWLINE
	call	WRAP_WORD
PUTC_PEND:
	movf	P_PEND,W

; Stores character W at P_COL of the current line and draws it.
PUTC_RAW:
	movwf	P_CH
	movf	P_CUR,W
	call	TXT_ADDR
	movf	P_COL,W
	addwf	SR0,F			; lines don't cross a 256-byte boundary
	C0	SR_OPEN_WR
	movf	P_CH,W
	C0	SPI_WR
	C0	SPI_CLOSE
	movf	P_CUR,W
	call	LOAD_LINE
	call	CUR_K
	rrf	P_COL,W
	movwf	P_W
	rrf	P_W,F
	movlw	0x07
	andwf	P_W,F
	call	RENDER_WORD
	incf	P_COL,F
	return

; P_K = the screen line of the current line, (P_CUR - P_TOP) mod 10.
CUR_K:
	movf	P_TOP,W
	subwf	P_CUR,W
	btfss	STATUS,C
	addlw	10
	movwf	P_K
	return

; The current line is full: if it has a space, the characters after the
; last one go to a new line (blanked and redrawn on this one); otherwise
; just a new line.
WRAP_WORD:
	movf	P_CUR,W
	call	LOAD_LINE
	movlw	31
	movwf	P_K2
WRAP_FIND:
	bsf	STATUS,IRP
	movf	P_K2,W
	addlw	LOW (TXTBUF)
	movwf	FSR
	movf	INDF,W
	xorlw	' '
	btfsc	STATUS,Z
	goto	WRAP_FOUND
	decf	P_K2,F
	btfss	P_K2,7
	goto	WRAP_FIND
	goto	NEWLINE			; no space: break the word
WRAP_FOUND:
	movf	P_K2,W
	xorlw	31
	btfsc	STATUS,Z
	goto	NEWLINE
	; move TXTBUF[k + 1 ..] to TAILBUF and blank it
	clrf	P_N
WRAP_COPY:
	incf	P_K2,W
	addwf	P_N,W
	addlw	LOW (TXTBUF)
	movwf	FSR
	movf	INDF,W
	movwf	T1
	movlw	' '
	movwf	INDF
	movf	P_N,W
	addlw	LOW (TAILBUF)
	movwf	FSR
	movf	T1,W
	movwf	INDF
	incf	P_N,F
	incf	P_K2,W
	addwf	P_N,W
	xorlw	32
	btfss	STATUS,Z
	goto	WRAP_COPY
	; write the line back and redraw its last words
	movf	P_CUR,W
	call	TXT_ADDR
	C0	SR_OPEN_WR
	bsf	STATUS,IRP
	movlw	LOW (TXTBUF)
	movwf	FSR
WRAP_WRITE:
	movf	INDF,W
	C0	SPI_WR
	incf	FSR,F
	movf	FSR,W
	xorlw	LOW (TXTBUF) + 32
	btfss	STATUS,Z
	goto	WRAP_WRITE
	C0	SPI_CLOSE
	call	CUR_K
	incf	P_K2,W			; from the word holding column k + 1
	movwf	P_W
	rrf	P_W,F
	rrf	P_W,F
	movlw	0x07
	andwf	P_W,F
WRAP_REDRAW:
	call	RENDER_WORD
	incf	P_W,F
	btfss	P_W,3
	goto	WRAP_REDRAW
	call	NEWLINE
	clrf	P_I2			; the word on the new line
WRAP_PUT:
	bsf	STATUS,IRP
	movf	P_I2,W
	addlw	LOW (TAILBUF)
	movwf	FSR
	movf	INDF,W
	call	PUTC_RAW
	incf	P_I2,F
	movf	P_I2,W
	xorwf	P_N,W
	btfss	STATUS,Z
	goto	WRAP_PUT
	return

; Text state (P_TOP, P_CUR, P_COL) to and from SRAM.
TXT_STATE_ADDR:
	movlw	LOW (TXT_STATE)
	movwf	SR0
	movlw	HIGH (TXT_STATE)
	movwf	SR1
	movlw	UPPER (TXT_STATE)
	movwf	SR2
	return

TXT_LOAD:
	call	TXT_STATE_ADDR
	C0	SR_OPEN_RD
	C0	SPI_RD
	movwf	P_TOP
	C0	SPI_RD
	movwf	P_CUR
	C0	SPI_RD
	movwf	P_COL
	C0	SPI_CLOSE
	return

TXT_SAVE:
	call	TXT_STATE_ADDR
	C0	SR_OPEN_WR
	movf	P_TOP,W
	C0	SPI_WR
	movf	P_CUR,W
	C0	SPI_WR
	movf	P_COL,W
	C0	SPI_WR
	C0	SPI_CLOSE
	return

; Clears the text in SRAM and the state: an empty screen.
TXT_INIT:
	clrf	P_TOP
	clrf	P_CUR
	clrf	P_COL
	clrf	P_L
TXT_INIT_LOOP:
	movf	P_L,W
	call	CLEAR_LINE
	incf	P_L,F
	movf	P_L,W
	xorlw	10
	btfss	STATUS,Z
	goto	TXT_INIT_LOOP
	goto	TXT_SAVE

; Prints the text of token TOKN (NEXTL/NEXTH) after token TOKL/TOKH: a
; leading space is dropped after BOS (as Tokenizer.piece does) and at the
; start of a line, and a piece that starts with a space and doesn't fit
; on the line goes to the next one.
PRINT_TOKEN:
	call	TXT_LOAD
	clrf	P_OFSL			; offset of the token's text: sum of the
	clrf	P_OFSH			; lengths of the tokens before it
	clrf	P_IL
	clrf	P_IH
PRINT_OFS:
	movf	P_IL,W
	xorwf	NEXTL,W
	btfss	STATUS,Z
	goto	PRINT_OFS_ADD
	movf	P_IH,W
	xorwf	NEXTH,W
	btfsc	STATUS,Z
	goto	PRINT_OFS_DONE
PRINT_OFS_ADD:
	call	VOCAB_LEN_I
	addwf	P_OFSL,F
	btfsc	STATUS,C
	incf	P_OFSH,F
	INCN	P_IL, 2
	goto	PRINT_OFS
PRINT_OFS_DONE:
	call	VOCAB_LEN_I
	movwf	P_LEN
	movf	P_LEN,F
	btfsc	STATUS,Z
	goto	TXT_SAVE
	call	VOCAB_CHAR
	xorlw	' '
	btfss	STATUS,Z
	goto	PRINT_LOOP
	; A leading space: drop it after BOS or at the start of a line, and
	; wrap if the piece doesn't fit.
	movf	TOKH,F
	btfss	STATUS,Z
	goto	PRINT_NOT_BOS
	decf	TOKL,W
	btfsc	STATUS,Z
	goto	PRINT_SKIP
PRINT_NOT_BOS:
	movf	P_COL,F
	btfsc	STATUS,Z
	goto	PRINT_SKIP
	movf	P_LEN,W			; fits if P_COL + P_LEN <= 32
	addwf	P_COL,W
	sublw	32
	btfsc	STATUS,C
	goto	PRINT_LOOP
	call	NEWLINE
PRINT_SKIP:
	INCN	P_OFSL, 2
	decf	P_LEN,F
	btfsc	STATUS,Z
	goto	TXT_SAVE
PRINT_LOOP:
	call	VOCAB_CHAR
	call	PUTC
	INCN	P_OFSL, 2
	decfsz	P_LEN,F
	goto	PRINT_LOOP
	goto	TXT_SAVE

; W = VOCAB_LEN[P_IH:P_IL]
VOCAB_LEN_I:
	movlw	LOW (VOCAB_LEN)
	addwf	P_IL,W
	movwf	FAL
	movlw	HIGH (VOCAB_LEN)
	btfsc	STATUS,C
	addlw	1
	addwf	P_IH,W
	movwf	FAH
	FRD
	return

; W = VOCAB_TXT[P_OFSH:P_OFSL]
VOCAB_CHAR:
	movlw	LOW (VOCAB_TXT)
	addwf	P_OFSL,W
	movwf	FAL
	movlw	HIGH (VOCAB_TXT)
	btfsc	STATUS,C
	addlw	1
	addwf	P_OFSH,W
	movwf	FAH
	FRD
	return
