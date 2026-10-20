/* =====================================================================
 * link.x — скрипт компоновки USBL-Rust (Huawei E3272s-153, Balong V7R1)
 *
 * Карта памяти (байт-в-байт как в usblsafe-327x.bin, docs/00 §3):
 *   IRAM  : 0x2FFE4000 .. 0x2FFE43BF   — стаб первого уровня (876 байт)
 *   SDRAM : 0x30004000 .. 0x301B018C   — тело загрузчика (1 753 484 байт)
 *              (это ограничение ФАЙЛА образа: только загружаемые секции)
 *   BSS   : 0x301B018C .. 0x301C8000   — за концом файла (как в оригинале:
 *              «за 0x301B018C — BSS/кучи — в файле нет», docs/00 §5)
 *
 * Формат образа (контракт bootrom):
 *   файл 0x000..0x053 — заголовок (84 байта, поля +0x28/+0x2C/+0x38/+0x3C/+0x40)
 *   файл 0x054..0x3BF — стаб   @ IRAM  0x2FFE4000, вход = начало (header +0x2C)
 *   файл 0x3C0..EOF   — тело   @ SDRAM 0x30004000, вход = начало (sysInit)
 *
 * Паковщик (tools/pack.py) добивает стаб до 876 байт и тело до 1 753 484
 * байт, поэтому заголовок остаётся ПАБАЙТОВО ИДЕНТИЧНЫМ оригиналу.
 * ===================================================================== */

ENTRY(stub_entry)

MEMORY
{
    IRAM   (rx)  : ORIGIN = 0x2FFE4000, LENGTH = 876          /* 0x36C  — header +0x28 */
    SDRAM  (rwx) : ORIGIN = 0x30004000, LENGTH = 1753484      /* 0x1AC18C — header +0x38 (файл) */
    /* BSS/стек/буфер образа — за концом файла (как в оригинале: «за 0x301B018C —
       BSS/кучи — в файле нет»). В burn-режиме модемные ядра не запущены,
       поэтому окно DDR 0x30000000..0x3FF00000 (маска стаба) доступно целиком;
       консервативно ограничиваем 16 МБ. */
    BSSRAM (rw)  : ORIGIN = 0x301B018C, LENGTH = 0x64FE74     /* до 0x30800000 */
}

SECTIONS
{
    /* ---------- Стаб (IRAM, исполняется bootrom'ом до загрузки тела) ---------- */
    .stub 0x2FFE4000 : ALIGN(4)
    {
        KEEP(*(.stub.entry))
        . = ALIGN(4);
    } > IRAM

    /* ---------- Тело (SDRAM, точка входа sysInit @ 0x30004000) ---------- */

    /* Векторы исключений ARM — первые 0x40 байт тела: вход тела = вектор reset,
     * что функционально эквивалентно оригинальному sysInit @ 0x30004000. */
    .vectors 0x30004000 : ALIGN(4)
    {
        KEEP(*(.vectors))
        . = ALIGN(4);
    } > SDRAM

    .text : ALIGN(4)
    {
        KEEP(*(.text.entry))        /* sysInit — сразу после векторов */
        *(.text .text.*)
        . = ALIGN(4);
    } > SDRAM

    .rodata : ALIGN(4)
    {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(4);
    } > SDRAM

    /* Данные, извлечённые из оригинального образа байт-в-байт */
    .origdata : ALIGN(4)
    {
        KEEP(*(.origdata .origdata.*))
        . = ALIGN(4);
    } > SDRAM

    .data : ALIGN(4)
    {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        . = ALIGN(4);
        __data_end = .;
    } > SDRAM

    /* ---------- BSS — за концом файла образа (как в оригинале) ---------- */
    .bss 0x301B018C (NOLOAD) : ALIGN(8)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    } > BSSRAM

    /* Стек растёт вниз от верха BSS-региона */
    .stack (NOLOAD) : ALIGN(8)
    {
        __stack_bottom = .;
        . += 0x6000;                /* 24 КиБ стека главного цикла */
        __stack_top = .;
    } > BSSRAM

    /DISCARD/ :
    {
        *(.ARM.exidx*)               /* нет unwind-таблиц: panic=abort */
        *(.ARM.attributes)
        *(.comment)
        *(.note*)
    }
}

/* Границы для паковщика и startup-кода */
PROVIDE(__stub_start = ORIGIN(IRAM));
PROVIDE(__stub_end   = ORIGIN(IRAM) + LENGTH(IRAM));
PROVIDE(__body_start = ORIGIN(SDRAM));
PROVIDE(__body_end   = ORIGIN(SDRAM) + LENGTH(SDRAM));
ASSERT(__bss_start >= __body_end, "bss должен начинаться за концом файла тела")
ASSERT(__stack_top <= 0x30800000, "стек/буферы не должны выходить за 8 МБ от базы тела")
