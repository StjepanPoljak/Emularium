; Bring-up for the TinyStories board: reads the three 25LC1024 EEPROMs
; holding the weight image, with bit-banged SPI (mode 0).
; Build: gpasm -p p16f876 -o eeprom_test.hex eeprom_test.asm
;
; For each EEPROM (CHIP 0..2):
; 1. RDID: the electronic signature into SIG0 + CHIP (expect 0x29).
; 2. RDSR: the status register into STAT0 + CHIP (expect 0: no write in
;    progress, writes disabled, no block protection).
; 3. READ: 16 bytes into BUFFER + 16 * CHIP, from address 0 of EE0 and EE1
;    (EE0 starts with the image header, "TS26") and from 0x001E30 of EE2,
;    which spans the end of the 269,880-byte image: 8 image bytes, then 8
;    erased ones (0xFF).
; Then sets DONE to 1 and loops.

	LIST P=16F876
	#include <p16f876.inc>
	__CONFIG _HS_OSC & _WDT_OFF & _PWRTE_ON & _BODEN_OFF & _LVP_ON & _CP_OFF

; The MSSP pins, so the hardware SPI can take over later; RC6 and RC7 stay
; free for the USART.
#define EE0_CS	PORTC,0
#define EE1_CS	PORTC,1
#define RAM_CS	PORTC,2
#define SCK	PORTC,3
#define MISO	PORTC,4
#define MOSI	PORTC,5
#define EE2_CS	PORTA,5

; 25LC1024 instructions
CMD_READ	EQU	0x03
CMD_RDSR	EQU	0x05
CMD_RDID	EQU	0xAB

READ_LEN	EQU	.16
; Where EE2 is read: 8 bytes before the end of the image.
EE2_ADDR_M	EQU	0x1E
EE2_ADDR_L	EQU	0x30

	CBLOCK	0x20
	DONE
	SIG0, SIG1, SIG2
	STAT0, STAT1, STAT2
	SPI_BYTE
	SPI_BITS
	CHIP
	I
	TMP
	ADDR_M
	ADDR_L
	ENDC
BUFFER		EQU	0x40		; 3 x READ_LEN bytes

	ORG	0
	GOTO	MAIN

; Sends W and returns the byte received in W. SI is sampled by the chip
; on the rising edge; it shifts out its next bit on the falling edge.
SPI_XFER:
	MOVWF	SPI_BYTE
	MOVLW	8
	MOVWF	SPI_BITS
SPI_BIT:
	BCF	MOSI
	BTFSC	SPI_BYTE,7
	BSF	MOSI
	BSF	SCK
	BCF	STATUS,C
	BTFSC	MISO
	BSF	STATUS,C
	RLF	SPI_BYTE,F		; bit 7 out, MISO in at bit 0
	BCF	SCK
	DECFSZ	SPI_BITS,F
	GOTO	SPI_BIT
	MOVF	SPI_BYTE,W
	RETURN

; Selects EEPROM number CHIP.
CS_LOW:
	MOVF	CHIP,W
	BTFSC	STATUS,Z
	BCF	EE0_CS
	XORLW	1
	BTFSC	STATUS,Z
	BCF	EE1_CS
	MOVF	CHIP,W
	XORLW	2
	BTFSC	STATUS,Z
	BCF	EE2_CS
	RETURN

; Deselects all EEPROMs.
CS_HIGH:
	BSF	EE0_CS
	BSF	EE1_CS
	BSF	EE2_CS
	RETURN

; Sends three zero bytes: the address of RDID, which only clocks it in.
ZERO_ADDR:
	MOVLW	0
	CALL	SPI_XFER
	MOVLW	0
	CALL	SPI_XFER
	MOVLW	0
	GOTO	SPI_XFER

; Stores W at TMP + CHIP.
STORE:
	MOVWF	SPI_BYTE
	MOVF	TMP,W
	ADDWF	CHIP,W
	MOVWF	FSR
	MOVF	SPI_BYTE,W
	MOVWF	INDF
	RETURN

MAIN:
	CLRF	DONE
	MOVLW	B'00100000'		; EE2_CS high; LCD RS, RW, E low
	MOVWF	PORTA
	CLRF	PORTB
	MOVLW	B'00000111'		; EE0_CS, EE1_CS, RAM_CS high, SCK low
	MOVWF	PORTC
	BSF	STATUS,RP0
	MOVLW	0x06			; PORTA digital
	MOVWF	ADCON1
	MOVLW	B'11011000'		; RA0-RA2 (LCD control), RA5 outputs
	MOVWF	TRISA
	CLRF	TRISB			; LCD data on RB4..RB7; RB3 is PGM
	MOVLW	B'11010000'		; RC0-RC3, RC5 outputs; RC4 (MISO) input
	MOVWF	TRISC
	BCF	STATUS,RP0

	CLRF	CHIP
CHIP_LOOP:
	; RDID: command, 3 address bytes, then the signature.
	MOVLW	SIG0
	MOVWF	TMP
	CALL	CS_LOW
	MOVLW	CMD_RDID
	CALL	SPI_XFER
	CALL	ZERO_ADDR
	MOVLW	0
	CALL	SPI_XFER
	CALL	CS_HIGH			; keeps W
	CALL	STORE

	; RDSR
	MOVLW	STAT0
	MOVWF	TMP
	CALL	CS_LOW
	MOVLW	CMD_RDSR
	CALL	SPI_XFER
	MOVLW	0
	CALL	SPI_XFER
	CALL	CS_HIGH
	CALL	STORE

	; READ_LEN bytes into BUFFER + 16 * CHIP.
	CLRF	ADDR_M
	CLRF	ADDR_L
	MOVF	CHIP,W
	XORLW	2
	BTFSS	STATUS,Z
	GOTO	READ_START
	MOVLW	EE2_ADDR_M
	MOVWF	ADDR_M
	MOVLW	EE2_ADDR_L
	MOVWF	ADDR_L
READ_START:
	CALL	CS_LOW
	MOVLW	CMD_READ
	CALL	SPI_XFER
	MOVLW	0
	CALL	SPI_XFER
	MOVF	ADDR_M,W
	CALL	SPI_XFER
	MOVF	ADDR_L,W
	CALL	SPI_XFER
	SWAPF	CHIP,W			; 16 * CHIP
	ADDLW	BUFFER
	MOVWF	FSR
	MOVLW	READ_LEN
	MOVWF	I
READ_LOOP:
	MOVLW	0
	CALL	SPI_XFER
	MOVWF	INDF
	INCF	FSR,F
	DECFSZ	I,F
	GOTO	READ_LOOP
	CALL	CS_HIGH

	INCF	CHIP,F
	MOVF	CHIP,W
	XORLW	3
	BTFSS	STATUS,Z
	GOTO	CHIP_LOOP

	MOVLW	1
	MOVWF	DONE
FINISHED:
	GOTO	FINISHED

	END
