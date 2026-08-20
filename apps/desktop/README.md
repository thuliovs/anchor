# Anchor Desktop

Primeira fatia vertical de comunicacao local do Anchor.

## Escopo atual

O desktop inicia um receptor UDP local em `127.0.0.1:57421`, valida `MotionSampleV1` do protocolo v1 e mantem o estado mais recente em memoria com metricas acumuladas.

Esta etapa ainda nao implementa:

- conexao com celular;
- sensores reais;
- overlay visual;
- autenticacao;
- criptografia.

## Como executar

Terminal 1:

```bash
pnpm dev:desktop
```

Terminal 2:

```bash
pnpm dev:simulator
```

Execucao por tempo limitado:

```bash
pnpm dev:simulator -- --duration 5 --pattern sine
```

## Endereco e porta padrao

- Host: `127.0.0.1`
- Porta: `57421`
- Taxa esperada do stream: `60 Hz`
- Limite tecnico de processamento: `240 datagramas por segundo`

O bind permanece apenas em loopback nesta etapa porque ainda nao existe emparelhamento nem autenticacao. Isso reduz a superficie de abuso enquanto o transporte ainda e exclusivamente local.

## Estado do receptor

O estado em memoria mantem:

- ultima amostra valida;
- emissor ativo;
- `sessionId` ativo;
- ultimo `sequence` aceito;
- instante local da ultima recepcao valida;
- metricas acumuladas.

## Status do stream

- `active`: recebeu amostra valida nos ultimos `250 ms`;
- `stale`: nao recebeu amostra valida por mais de `250 ms`;
- `disconnected`: nao recebeu amostra valida por mais de `1 s`.

A ultima amostra nao e apagada imediatamente quando o stream fica `stale` ou `disconnected`.

## Regras de sessao e ordenacao

- a primeira amostra valida estabelece a sessao ativa;
- a mesma sessao so aceita `sequence` estritamente crescente;
- pacotes duplicados ou fora de ordem sao ignorados e contabilizados;
- outra sessao so pode assumir depois de `1 s` sem amostra valida;
- rollover de `u32` nao e tratado neste MVP e permanece uma decisao deliberada documentada.

## Metricas

- `received_datagrams`: datagramas recebidos pelo socket;
- `accepted_samples`: amostras validas aceitas no estado;
- `oversized_datagrams`: datagramas acima de `1024` bytes;
- `invalid_packets`: JSON invalido, estrutura invalida ou validacao semantica rejeitada;
- `duplicate_or_out_of_order_packets`: sequencias repetidas ou antigas;
- `foreign_session_packets`: pacotes ignorados por pertencerem a outra sessao enquanto a atual segue ativa;
- `rate_limited_datagrams`: pacotes descartados pelo limitador tecnico antes do parse.

Os logs fazem resumo dessas metricas no maximo uma vez por segundo. Nao ha log por pacote.

## Estrutura Rust

```text
apps/desktop/src-tauri/src/
├── lib.rs
├── main.rs
├── protocol.rs
└── receiver/
    ├── mod.rs
    └── udp.rs
```
