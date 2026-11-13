; SPI, bit-banged in mode 0 on the MSSP pins (RC3 SCK, RC4 MISO, RC5 MOSI),
; with one chip select per chip. Runs in bank 0.
;
; The weight image is one byte stream over the three EEPROMs: SA2:SA1:SA0
; is the image address of the next byte. EE_RD opens the stream at SA when
; it isn't open (FLAGS.EEOPEN clear), so any other SPI traffic just closes
; it, and crossing into the next chip (every 128K) reopens it there.

; Deselects every chip; the EEPROM stream will reopen at SA.
SPI_CLOSE:
	bsf	EE0_CS
	bsf	EE1_CS
	bsf	EE2_CS
	bsf	RAM_CS
	bcf	FLAGS,EEOPEN
	return

; Sends W. Uses SPIB.
SPI_WR:
	movwf	SPIB
	variable	v_b
v_b = 7
	while v_b >= 0
	bcf	MOSI
	btfsc	SPIB,v_b
	bsf	MOSI
	bsf	SCK
	bcf	SCK
v_b = v_b - 1
	endw
	return

; Receives a byte into W (and SPIB); MOSI is left as it is.
SPI_RD:
	clrf	SPIB
v_b = 7
	while v_b >= 0
	bsf	SCK
	btfsc	MISO
	bsf	SPIB,v_b
	bcf	SCK
v_b = v_b - 1
	endw
	movf	SPIB,W
	return

; Opens a READ of the EEPROM holding image address SA, at that address.
EE_SEEK:
	call	SPI_CLOSE
	movf	SA2,W
	andlw	0x06			; chip = SA >> 17
	btfsc	STATUS,Z
	bcf	EE0_CS
	xorlw	0x02
	btfsc	STATUS,Z
	bcf	EE1_CS
	movf	SA2,W
	andlw	0x06
	xorlw	0x04
	btfsc	STATUS,Z
	bcf	EE2_CS
	movlw	0x03			; READ
	call	SPI_WR
	movf	SA2,W
	andlw	0x01
	call	SPI_WR
	movf	SA1,W
	call	SPI_WR
	movf	SA0,W
	call	SPI_WR
	bsf	FLAGS,EEOPEN
	return

; Next byte of the weight stream into W.
EE_RD:
	btfss	FLAGS,EEOPEN
	call	EE_SEEK
	clrf	SPIB
v_b = 7
	while v_b >= 0
	bsf	SCK
	btfsc	MISO
	bsf	SPIB,v_b
	bcf	SCK
v_b = v_b - 1
	endw
	incfsz	SA0,F
	goto	EE_RD_DONE
	incf	SA1,F
	btfss	STATUS,Z
	goto	EE_RD_DONE
	incf	SA2,F
	btfss	SA2,0			; even: the next chip starts here
	bcf	FLAGS,EEOPEN
EE_RD_DONE:
	movf	SPIB,W
	return

; Opens an SRAM transfer at SR2:SR1:SR0; W is the command (SR_READ or
; SR_WRITE). Closes the EEPROM stream.
SR_OPEN:
	movwf	T1
	call	SPI_CLOSE
	bcf	RAM_CS
	movf	T1,W
	call	SPI_WR
	movf	SR2,W
	call	SPI_WR
	movf	SR1,W
	call	SPI_WR
	movf	SR0,W
	goto	SPI_WR

SR_OPEN_RD:
	movlw	SR_READ
	goto	SR_OPEN

SR_OPEN_WR:
	movlw	SR_WRITE
	goto	SR_OPEN

; Reads 3 bytes from the open SRAM stream into A, sign-extended to 32 bits.
SR_RD24:
	call	SPI_RD
	movwf	A0
	call	SPI_RD
	movwf	A1
	call	SPI_RD
	movwf	A2
	goto	SEXT24

; Writes A0..A2 to the open SRAM stream.
SR_WR24:
	movf	A0,W
	call	SPI_WR
	movf	A1,W
	call	SPI_WR
	movf	A2,W
	goto	SPI_WR

; SR += 3
SR_ADD3:
	movlw	3
	addwf	SR0,F
	btfss	STATUS,C
	return
	incf	SR1,F
	btfsc	STATUS,Z
	incf	SR2,F
	return
