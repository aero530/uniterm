import { invoke } from "@tauri-apps/api/tauri";

import type {Connection, ConfigType, LogSettingsType} from './stores';
import { ports } from './stores.js';

function closeLink(id: string) {
    invoke("close_connection", {id: id})
    .then((data) => {
        ports.setIsActive(id, false);
    })
    .catch((error) => {
        alert(error);
        console.error(error);
    });
}

export function checkError(port: Connection, error: string) {
    if (error.includes("Lost coms with serial port.")) {
        closeLink(port.id);
    }
}

export function sendClear(port: Connection) {
    invoke("send_message", {id: port.id, message: {command: "Clear", package: ""}})
    .then(() => {})
    .catch((error) => {
        alert(error);
        console.error(error);
        checkError(port, error);
    });
}

export function sendAsciiCode(port: Connection, byte: number) {
    let ok_to_send = true;

    if (byte>=256) {
        ok_to_send = false;
        alert("Can not have value greater than 0xFF (255).")
    }

    if (ok_to_send) {
        invoke("send_message", {id: port.id, message: {command: "Tx", package: byte}})
        .then(() => {})
        .catch((error) => {
            alert(error);
            console.error(error);
            checkError(port, error);
        });
    }
}

export function sendCommand(port: Connection, char: String, sendMode: String, cr: boolean, lf: boolean) {
    let data;
    let ok_to_send = true;

    if (sendMode == 'Ascii') {
        data = char.concat(cr?"\r":"",lf?"\n":"");
    } else if (sendMode == 'Decimal') {
        data = char.split(/[\s,,]/).map((x) => {
            let n = parseInt(x,10);
            if (n>=256) {
                ok_to_send = false;
                alert("Data is send as bytes so decimal values must be less than or equal to 255.")
            }
            return n
        });
        if (cr) {data.push(0x0d)};
        if (lf) {data.push(0x0a)};
    } else if (sendMode == 'Hex') {
        data = char.split(/[\s,,]/).map((x) => {
            let n = parseInt(x,16);
            if (n>=256) {
                ok_to_send = false;
                alert("Can not have hex values greater than 0xFF (255).  Multiple bytes must be seperated by comma or space.")
            }
            return n
        });
        if (cr) {data.push(0x0d)};
        if (lf) {data.push(0x0a)};
    }

    if (ok_to_send) {
        invoke("send_message", {id: port.id, message: {command: "Tx", package: data}})
        .then(() => {})
        .catch((error) => {
            alert(error);
            console.error(error);
            checkError(port, error);
        });
    }
}

export function sendDisplayConfig(port: Connection, data: ConfigType) {
    if (port.is_active) {
        invoke("send_message", {id: port.id, message: {command: "Settings", package: data}})
        .then(() => {})
        .catch((error) => {
            alert(error);
            console.error(error);
            checkError(port, error);
        });
    }
}

export function sendLogSettings(port: Connection, data: LogSettingsType) {
    if (port.is_active) {
        invoke("send_message", {id: port.id, message: {command: "Logging", package: data}})
        .then(() => {

        })
        .catch((error) => {
            alert(error);
            console.error(error);
            checkError(port, error);
        });
    }
}