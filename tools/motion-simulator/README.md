# @anchor/motion-simulator

Simulador local em TypeScript para enviar `MotionSampleV1` por UDP ao desktop.

## Como executar

```bash
pnpm dev:simulator
```

Com duracao limitada:

```bash
pnpm dev:simulator -- --duration 5 --pattern sine
```

## Opcoes

- `--host` padrao `127.0.0.1`
- `--port` padrao `57421`
- `--rate` padrao `60` e intervalo permitido `1..120`
- `--pattern` padrao `sine`
- `--duration` em segundos; omitido para execucao continua

## Padroes disponiveis

### `stationary`

- aceleracao linear nula;
- gravidade fixa em `z = -9.80665`;
- velocidade angular nula.

### `sine`

- aceleracao lateral senoidal em `x`;
- aceleracao longitudinal senoidal mais lenta em `y`;
- gravidade estavel em `z = -9.80665`;
- velocidade angular suave em `z`.

## Comportamento

- usa `node:dgram`;
- gera um novo `sessionId` por execucao;
- inicia `sequence` em `0`;
- calcula `sessionElapsedUs` com relogio monotonic do processo;
- verifica que o payload JSON UTF-8 cabe em `1024` bytes;
- imprime apenas mensagem inicial e resumo final.

O receptor desktop agora faz bind em `0.0.0.0:57421`, entao o simulador continua funcionando em `127.0.0.1:57421` sem nenhuma mudanca de uso.
