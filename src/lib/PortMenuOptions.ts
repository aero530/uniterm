
export let baudRates = [
    { id: 1, text: "300 baud", value: 300 },
    { id: 2, text: "600 baud", value: 600 },
    { id: 3, text: "1200 baud", value: 1200 },
    { id: 4, text: "1800 baud", value: 1800 },
    { id: 5, text: "2400 baud", value: 2400 },
    { id: 6, text: "4000 baud", value: 4000 },
    { id: 7, text: "4800 baud", value: 4800 },
    { id: 8, text: "7200 baud", value: 7200 },
    { id: 9, text: "9600 baud", value: 9600 },
    { id: 10, text: "14.4 kbaud", value: 14400 },
    { id: 11, text: "16.0 kbaud", value: 16000 },
    { id: 12, text: "19.2 kbaud", value: 19200 },
    { id: 13, text: "28.8 kbaud", value: 28800 },
    { id: 14, text: "38.4 kbaud", value: 38400 },
    { id: 15, text: "51.2 kbaud", value: 51200 },
    { id: 16, text: "56.0 kbaud", value: 56000 },
    { id: 18, text: "64.0 kbaud", value: 64000 },
    { id: 17, text: "57.6 kbaud", value: 57600 },
    { id: 19, text: "76.8 kbaud", value: 76800 },
    { id: 20, text: "115.2 kbaud", value: 115200 },
    { id: 21, text: "128.0 kbaud", value: 128000 },
    { id: 22, text: "153.6 kbaud", value: 153600 },
    { id: 23, text: "230.4 kbaud", value: 230400 },
    { id: 24, text: "250.0 kbaud", value: 250000 },
    { id: 25, text: "256.0 kbaud", value: 256000 },
    { id: 26, text: "460.8 kbaud", value: 460800 },
    { id: 27, text: "500.0 kbaud", value: 500000 },
    { id: 28, text: "576.0 kbaud", value: 576000 },
    { id: 29, text: "921.6 kbaud", value: 921600 },
    { id: 30, text: "1.00 Mbaud", value: 1000000 },
    { id: 31, text: "1.20 Mbaud", value: 1200000 },
    { id: 32, text: "1.50 Mbaud", value: 1500000 },
    { id: 33, text: "2.00 Mbaud", value: 2000000 },
    { id: 34, text: "2.25 Mbaud", value: 2250000 },
    { id: 35, text: "3.00 Mbaud", value: 3000000 },
    { id: 36, text: "4.50 Mbaud", value: 4500000 },
];

export let flowControl = [
    { id: 1, text: `No Flow Ctrl`, value: 'None' },
    { id: 2, text: `Software`, value: 'Software' },
    { id: 3, text: `Hardware`, value: 'Hardware' }
];

export let dataBits = [
    { id: 1, text: `Five Data Bits`, value: 'Five' },
    { id: 2, text: `Six Data Bits`, value: 'Six' },
    { id: 3, text: `Seven Data Bits`, value: 'Seven' },
    { id: 4, text: `Eight Data Bits`, value: 'Eight' },
];

export let parity = [
    { id: 1, text: `No Parity`, value: 'None' },
    { id: 2, text: `Odd Parity`, value: 'Odd' },
    { id: 3, text: `Even Parity`, value: 'Even' }
];

export let stopBits = [
    { id: 1, text: `One Stop Bit`, value: 'One' },
    { id: 2, text: `Two Stop Bits`, value: 'Two' },
];

export let displayMode = [
    { id: 1, text: `ASCII`, value: 'Ascii' },
    { id: 2, text: `ANSI`, value: 'Ansi' },
    { id: 3, text: `Decimal`, value: "Decimal" },
    { id: 4, text: `Hex`, value: 'Hex' }
];

export let sendMode = [
    { id: 1, text: `Ascii`, value: 'Ascii' },
    { id: 2, text: `Decimal`, value: "Decimal" },
    { id: 3, text: `Hex`, value: 'Hex' }
];