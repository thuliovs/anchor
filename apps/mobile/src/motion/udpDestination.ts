export interface UdpDestination {
  host: string;
  port: number;
}

export type DestinationValidationCode =
  | 'ERR_INVALID_DESTINATION'
  | 'ERR_LOOPBACK_DESTINATION';

export interface DestinationValidationResult {
  ok: boolean;
  code: DestinationValidationCode | null;
  message: string | null;
  destination: UdpDestination | null;
  loopbackWarning: string | null;
}

const IPV4_PATTERN = /^(\d{1,3})(?:\.(\d{1,3})){3}$/;

export function validateUdpDestination(host: string, port: number): DestinationValidationResult {
  const normalizedHost = host.trim();
  if (!IPV4_PATTERN.test(normalizedHost)) {
    return invalidDestination('Informe um IPv4 valido do computador.');
  }

  const octets = normalizedHost.split('.').map(part => Number(part));
  if (octets.some(octet => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
    return invalidDestination('Informe um IPv4 valido do computador.');
  }

  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    return invalidDestination('A porta deve ser um inteiro entre 1 e 65535.');
  }

  if (octets[0] === 0) {
    return invalidDestination('Enderecos 0.0.0.0/8 nao sao aceitos nesta versao.');
  }

  if (normalizedHost === '255.255.255.255') {
    return invalidDestination('Broadcast global nao e aceito nesta versao.');
  }

  if (octets[0] >= 224 && octets[0] <= 239) {
    return invalidDestination('Enderecos multicast nao sao aceitos nesta versao.');
  }

  if (octets[0] >= 240) {
    return invalidDestination('Enderecos reservados 240.0.0.0/4 nao sao aceitos nesta versao.');
  }

  const loopbackWarning = octets[0] === 127
    ? '127.0.0.1 no celular aponta para o proprio celular, nao para o computador.'
    : null;

  return {
    ok: true,
    code: loopbackWarning ? 'ERR_LOOPBACK_DESTINATION' : null,
    message: null,
    destination: {
      host: normalizedHost,
      port,
    },
    loopbackWarning,
  };
}

function invalidDestination(message: string): DestinationValidationResult {
  return {
    ok: false,
    code: 'ERR_INVALID_DESTINATION',
    message,
    destination: null,
    loopbackWarning: null,
  };
}
