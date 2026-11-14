; Serial loader for the TinyStories board: writes the weight image into the
; three 25LC1024 EEPROMs from a host, over the USART (RC6 TX, RC7 RX) at
; 115,200 baud (113,636 at 20 MHz: SPBRG 10, BRGH), 8N1. load_weights.py
; is the host side.
; Build: gpasm -p p16f876 -o loader.hex loader.asm
;
; The host sends a command byte and waits for the reply:
;   '?'                      -> "EMLD1"
;   'W' A2 A1 A0 D[256] C C  -> 'K', or 'E' when the CRC didn't match
;   'R' A2 A1 A0             -> D[256] C C
;   anything else            -> '!'
; A2:A1:A0 is an image address, 256-aligned for W (one EEPROM page; the
; chip is A2 >> 1, as in the TinyStories firmware). C C is the CRC-16/
; CCITT-FALSE (polynomial 0x1021, initial value 0xFFFF) of D, high byte
; first. W streams each byte to the EEPROM as it comes, so a page with a
; bad CRC is written too: the host sends it again. W answers when the
; write cycle is over.
;
; Timing: a byte arrives every 88 us (440 instruction cycles); the SPI
; write and the CRC take about 180, so the USART's two-byte FIFO never
; overflows.

	LIST P=16F876
	#include <p16f876.inc>
	__CONFIG _HS_OSC & _WDT_OFF & _PWRTE_ON & _BODEN_OFF & _LVP_OFF & _CP_OFF

#define EE0_CS	PORTC,0
#define EE1_CS	PORTC,1
#define RAM_CS	PORTC,2
#define SCK	PORTC,3
#define MISO	PORTC,4
#define MOSI	PORTC,5
#define EE2_CS	PORTA,5

; 25LC1024 instructions
CMD_WRITE	EQU	0x02
CMD_READ	EQU	0x03
CMD_RDSR	EQU	0x05
CMD_WREN	EQU	0x06

	CBLOCK	0x20
	SPI_BYTE
	SPI_BITS
	CHIP
	A2, A1, A0
	N
	DATA
	CRCH, CRCL
	CRCN
	OK
	ENDC

	ORG	0
	goto	MAIN

; Sends W over SPI (mode 0) and returns the byte received in W.
SPI_XFER:
	movwf	SPI_BYTE
	movlw	8
	movwf	SPI_BITS
SPI_BIT:
	bcf	MOSI
	btfsc	SPI_BYTE,7
	bsf	MOSI
	bsf	SCK
	bcf	STATUS,C
	btfsc	MISO
	bsf	STATUS,C
	rlf	SPI_BYTE,F
	bcf	SCK
	decfsz	SPI_BITS,F
	goto	SPI_BIT
	movf	SPI_BYTE,W
	return

; Selects EEPROM CHIP (0..2).
CS_LOW:
	movf	CHIP,W
	btfsc	STATUS,Z
	bcf	EE0_CS
	xorlw	1
	btfsc	STATUS,Z
	bcf	EE1_CS
	movf	CHIP,W
	xorlw	2
	btfsc	STATUS,Z
	bcf	EE2_CS
	return

CS_HIGH:
	bsf	EE0_CS
	bsf	EE1_CS
	bsf	EE2_CS
	return

; Waits for a byte from the host; returns it in W. An overrun (only
; possible if the host doesn't wait for replies) is cleared.
RX_BYTE:
	btfss	RCSTA,OERR
	goto	RX_WAIT
	bcf	RCSTA,CREN
	bsf	RCSTA,CREN
RX_WAIT:
	btfss	PIR1,RCIF
	goto	RX_BYTE
	movf	RCREG,W
	return

; Sends W to the host.
TX_BYTE:
	btfss	PIR1,TXIF
	goto	TX_BYTE
	movwf	TXREG
	return

CRC_INIT:
	movlw	0xff
	movwf	CRCH
	movwf	CRCL
	return

; CRCH:CRCL = CRC-16/CCITT of the bytes so far and W.
CRC_BYTE:
	xorwf	CRCH,F
	movlw	8
	movwf	CRCN
CRC_BIT:
	bcf	STATUS,C
	rlf	CRCL,F
	rlf	CRCH,F
	btfss	STATUS,C
	goto	CRC_NEXT
	movlw	0x10
	xorwf	CRCH,F
	movlw	0x21
	xorwf	CRCL,F
CRC_NEXT:
	decfsz	CRCN,F
	goto	CRC_BIT
	return

; Receives A2 A1 A0; CHIP = A2 >> 1.
RX_ADDR:
	call	RX_BYTE
	movwf	A2
	call	RX_BYTE
	movwf	A1
	call	RX_BYTE
	movwf	A0
	bcf	STATUS,C
	rrf	A2,W
	movwf	CHIP
	return

; Selects the chip and sends W (the instruction) and the address.
SEND_CMD_ADDR:
	movwf	DATA
	call	CS_LOW
	movf	DATA,W
	call	SPI_XFER
	movf	A2,W
	andlw	1
	call	SPI_XFER
	movf	A1,W
	call	SPI_XFER
	movf	A0,W
	goto	SPI_XFER

MAIN:
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
	movlw	B'11010000'		; RC0-RC3, RC5 outputs; RC4 (MISO), RC6, RC7 inputs
	movwf	TRISC
	movlw	.10			; 113,636 baud at 20 MHz
	movwf	SPBRG
	movlw	B'00100100'		; TXEN, BRGH
	movwf	TXSTA
	bcf	STATUS,RP0
	movlw	B'10010000'		; SPEN, CREN
	movwf	RCSTA

COMMAND:
	call	RX_BYTE
	movwf	DATA
	xorlw	'?'
	btfsc	STATUS,Z
	goto	HELLO
	movf	DATA,W
	xorlw	'W'
	btfsc	STATUS,Z
	goto	WRITE
	movf	DATA,W
	xorlw	'R'
	btfsc	STATUS,Z
	goto	READ
	movlw	'!'
	call	TX_BYTE
	goto	COMMAND

HELLO:
	movlw	'E'
	call	TX_BYTE
	movlw	'M'
	call	TX_BYTE
	movlw	'L'
	call	TX_BYTE
	movlw	'D'
	call	TX_BYTE
	movlw	'1'
	call	TX_BYTE
	goto	COMMAND

WRITE:
	call	RX_ADDR
	call	CS_LOW
	movlw	CMD_WREN
	call	SPI_XFER
	call	CS_HIGH
	movlw	CMD_WRITE
	call	SEND_CMD_ADDR
	call	CRC_INIT
	clrf	N			; 256 bytes
WRITE_BYTE:
	call	RX_BYTE
	movwf	DATA
	call	SPI_XFER
	movf	DATA,W
	call	CRC_BYTE
	decfsz	N,F
	goto	WRITE_BYTE
	call	CS_HIGH			; starts the write cycle
	clrf	OK
	call	RX_BYTE
	xorwf	CRCH,W
	btfss	STATUS,Z
	incf	OK,F
	call	RX_BYTE
	xorwf	CRCL,W
	btfss	STATUS,Z
	incf	OK,F
WRITE_WAIT:
	call	CS_LOW
	movlw	CMD_RDSR
	call	SPI_XFER
	movlw	0
	call	SPI_XFER
	call	CS_HIGH			; keeps W
	andlw	0x01			; WIP
	btfss	STATUS,Z
	goto	WRITE_WAIT
	movlw	'K'
	movf	OK,F
	btfss	STATUS,Z
	movlw	'E'
	call	TX_BYTE
	goto	COMMAND

READ:
	call	RX_ADDR
	movlw	CMD_READ
	call	SEND_CMD_ADDR
	call	CRC_INIT
	clrf	N
READ_BYTE:
	movlw	0
	call	SPI_XFER
	movwf	DATA
	call	TX_BYTE
	movf	DATA,W
	call	CRC_BYTE
	decfsz	N,F
	goto	READ_BYTE
	call	CS_HIGH
	movf	CRCH,W
	call	TX_BYTE
	movf	CRCL,W
	call	TX_BYTE
	goto	COMMAND

	END
