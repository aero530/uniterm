import { writable } from 'svelte/store';
import { v4 as uuid } from 'uuid';

export type SerialPortType = {
    name: string,
    port_type: string,
    product: string,
    serial_number: string,
    manufacturer: string,
};

function createSerialList() {
	const { subscribe, set } = writable([]);

	return {
		subscribe,
        set: (input: [SerialPortType]) => set(input),
		reset: () => set([])
	};
}
export const serial_list = createSerialList();


export type ConnectionParams = {
    name: string,
    baud_rate: number,
}

export type Connection = {
    id: string,
    name: string,
    baud_rate: number,
    flow_control: string,
    data_bits: string,
    parity: string,
    stop_bits: string,
    max_bytes: number,
	display_mode: string,
	is_active: boolean,
	is_running: boolean,
	is_logging: boolean,
	log_path: string,
	rx_buffer: string,
};

export type ConfigType = {
	max_bytes: number,
	display_mode: string,
}

export type LogSettingsType = {
	enabled: boolean,
	path: string,
}

let defaultConnections : Connection[]|null = null;

function createConnection() {
	const { subscribe, set, update } = writable(defaultConnections);

	return {
		subscribe,
        set: (input: Connection[]) => set(input),
		reset: () => set(null),

		addPort: (params: ConnectionParams) => {
			let id = uuid();
            update(current => {
				let new_connection = {
					id: id,
					name: params.name,
					baud_rate: params.baud_rate,
					flow_control: 'None',
					data_bits: 'Eight',
					parity: 'None',
					stop_bits: 'One',
					max_bytes: 10000,
					display_mode: 'Ascii',
					is_active: false,
					is_running: false,
					is_logging: false,
					log_path: '',
					rx_buffer: '',
				}
				if (current === null) {
					current = [new_connection];
				} else {
					current.push(new_connection);
				}
                
                return current;
            });
        },
		removePort: (id: string) => {
            update(current => {
                current = current.filter(port => port.id != id);
				return current;
            });
        },
		setIsActive: (id: string, isOpen: boolean) => {
            update(current => {
				let index = current.findIndex((port => port.id == id));
                current[index].is_active = isOpen;
				current[index].rx_buffer = '';
                return current;
            });
        },
		setIsRunning: (id: string, isRunning: boolean) => {
            update(current => {
				let index = current.findIndex((port => port.id == id));
                current[index].is_running = isRunning;
				current[index].rx_buffer = '';
                return current;
            });
        },
		// setPort: (id: string, name: string) => {
        //     update(current => {
		// 		let index = current.findIndex((port => port.id == id));
        //         current[index].name = name;
        //         return current;
        //     });
        // },
		// setBaud: (id: string, baud_rate: number) => {
        //     update(current => {
		// 		let index = current.findIndex((port => port.id == id));
        //         current[index].baud_rate = baud_rate;
        //         return current;
        //     });
        // },
		setDisplayMode: (id: string, display_mode: string) => {
            update(current => {
				let index = current.findIndex((port => port.id == id));
                current[index].display_mode = display_mode;
                return current;
            });
        },
		setDisplaySize: (id: string, max_bytes: number) => {
            update(current => {
				let index = current.findIndex((port => port.id == id));
                current[index].max_bytes = max_bytes;
                return current;
            });
        },
		setLogEnabled: (id: string, enable: boolean) => {
            update(current => {
				let index = current.findIndex((port => port.id == id));
                current[index].is_logging = enable;
                return current;
            });
        },
		setLogFile: (id: string, path: string) => {
            update(current => {
				let index = current.findIndex((port => port.id == id));
                current[index].log_path = path;
                return current;
            });
        },
		updateRxBuffer: (id: string, rx_string: string) => {
			update(current => {
				let index = current.findIndex((port => port.id == id));
				console.log(rx_string);
				
				current[index].rx_buffer += rx_string; // add new data to the end of the buffer
				console.log(current[index].rx_buffer);

				// remove old data if the array is too long
				if (current[index].rx_buffer.length > current[index].max_bytes) {
					current[index].rx_buffer = current[index].rx_buffer.slice(-current[index].max_bytes); // pull the last max_bytes of data
				}
				console.log("rx buffer length "+current[index].rx_buffer.length);
				return current;
			});
		},
		setRxBuffer: (id: string, rx_buffer: string) => {
			update(current => {
				let index = current.findIndex((port => port.id == id));
				current[index].rx_buffer = rx_buffer;
				return current;
			});
		},
	};
}
export const ports = createConnection();