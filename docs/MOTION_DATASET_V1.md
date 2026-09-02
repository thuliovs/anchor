# Motion Dataset v1

Formato NDJSON versionado para a fase B1 do Anchor.

## Objetivo

O dataset v1 existe para registrar amostras brutas aceitas pelo receptor Rust e caracterizar o sinal observado sem calibracao, sem filtros e sem correcao de movimento.

Ele preserva dois relogios monotonicamente relativos:

- `sessionElapsedUs`: produzido no Android e relativo ao inicio da sessao mobile;
- `receivedElapsedUs`: produzido no desktop e relativo ao inicio da gravacao local.

Esses relogios nao devem ser comparados como clocks absolutos entre dispositivos.

## Escopo

O gravador escreve apenas amostras que ja passaram por:

1. recepcao do datagrama;
2. limite de tamanho;
3. validacao do protocolo;
4. rate limit;
5. regras de sessao e sequencia;
6. aceitacao efetiva da amostra.

Nao sao gravados datagramas invalidos, oversized, rate-limited, duplicados, fora de ordem ou de sessao estrangeira ainda nao aceita.

## Comandos

```bash
pnpm motion:record -- --scenario stationary --duration-seconds 15
pnpm motion:analyze -- artifacts/motion-datasets/<arquivo>.ndjson
pnpm motion:analyze -- artifacts/motion-datasets/<arquivo>.ndjson --json
```

## Diretorio padrao

Quando `--output` nao e informado, o gravador usa:

```text
artifacts/motion-datasets/
```

Nome padrao:

```text
YYYYMMDDThhmmssZ-<scenario>.ndjson
```

Exemplo:

```text
artifacts/motion-datasets/20260902T120000Z-stationary.ndjson
```

## Metadata

A primeira linha deve ser um objeto `metadata`:

```json
{
  "recordType": "metadata",
  "datasetFormatVersion": 1,
  "protocolVersion": 1,
  "scenario": "stationary",
  "startedAtUtc": "2026-09-02T12:00:00Z",
  "expectedSampleRateHz": 60,
  "mountingConvention": "flat_screen_up_portrait_top_toward_vehicle_front"
}
```

Campos:

- `datasetFormatVersion`: versao do formato do dataset, independente do protocolo UDP.
- `protocolVersion`: deve refletir a versao do `MotionSampleV1` gravado.
- `scenario`: nome controlado e seguro para filename.
- `startedAtUtc`: apenas informacao operacional.
- `expectedSampleRateHz`: taxa esperada da captura Android nesta fase.
- `mountingConvention`: convencao fisica assumida nesta fase.
- `notes`: opcional.

## Amostras

Cada linha `sample` preserva a amostra `MotionSampleV1` sem renomear campos:

```json
{
  "recordType": "sample",
  "receivedElapsedUs": 123456,
  "sample": {
    "protocolVersion": 1,
    "kind": "motion_sample",
    "sessionId": "550e8400-e29b-41d4-a716-446655440000",
    "sequence": 42,
    "sessionElapsedUs": 700000,
    "linearAccelerationMps2": {
      "x": 0,
      "y": 0,
      "z": 0
    },
    "gravityMps2": {
      "x": 0,
      "y": 0,
      "z": -9.80665
    },
    "angularVelocityRadS": {
      "x": 0,
      "y": 0,
      "z": 0
    }
  }
}
```

## Summary

Uma finalizacao limpa escreve uma ultima linha `summary`:

```json
{
  "recordType": "summary",
  "completed": true,
  "durationUs": 15000000,
  "receivedAcceptedSamples": 900,
  "writtenSamples": 900,
  "recorderDroppedSamples": 0
}
```

Sem `summary`, o arquivo deve ser tratado como potencialmente interrompido.

`completed=false` indica encerramento controlado antes da duracao prevista, por exemplo em `Ctrl+C`.

## Convencao de montagem assumida

Nesta fase ainda nao existe calibracao. A hipotese operacional atual e:

- telefone deitado;
- tela para cima;
- modo retrato;
- borda superior apontando para a frente do veiculo.

Sob essa convencao assumida:

- `X+ = direita`
- `Y+ = frente`
- `Z+ = cima`

Os datasets da B1 existem para verificar essa hipotese antes de qualquer operacao de zero/calibracao.

## Analise offline

O analisador informa:

- versao do dataset;
- cenario e convencao de montagem;
- completo ou interrompido;
- quantidade de amostras;
- sessoes encontradas;
- primeira e ultima sequencia por sessao;
- lacunas de sequencia;
- duracao observada;
- taxa media da origem;
- taxa media de recepcao;
- intervalos de origem;
- intervalos de recepcao;
- variacao relativa `receiveDeltaUs - sourceDeltaUs`;
- `recorderDroppedSamples`;
- estatisticas por eixo;
- magnitude media e desvio-padrao da gravidade.

## Definicoes estatisticas

Media:

```text
mean = sum(values) / n
```

Desvio-padrao:

```text
stddev = sqrt(sum((x - mean)^2) / n)
```

O dataset v1 usa desvio-padrao populacional, nao amostral.

Taxa media por intervalos:

```text
averageRateHz = intervalCount * 1_000_000 / sum(intervalUs)
```

Metodo de percentil:

- ordenar os valores em ordem crescente;
- usar o metodo linear R-7;
- rank zero-based = `p * (n - 1)`;
- interpolar linearmente entre `floor(rank)` e `ceil(rank)`.

Exemplo com `[100000, 200000]`:

- `p50 = 150000`
- `p95 = 195000`
- `p99 = 199000`

## Interpretacao temporal

As metricas temporais desta fase medem:

- intervalo entre amostras na origem;
- intervalo entre recepcoes no desktop;
- variacao relativa entre esses dois intervalos.

Elas nao medem latencia absoluta ponta a ponta.

## Backpressure e perda explicita

O gravador usa uma fila limitada entre receptor e escrita em disco.

- o loop UDP publica por operacao nao bloqueante;
- se a fila encher, o receptor continua funcionando;
- a perda e contabilizada em `recorderDroppedSamples`;
- `receivedAcceptedSamples` pode ser maior que `writtenSamples`.

## Checklist fisico seguro da B1

Checklist executado e aprovado em `2026-09-02` com Android real, desktop Linux e Wi-Fi local.

Os datasets brutos fisicos permanecem em `artifacts/motion-datasets/`, continuam ignorados pelo Git e nao devem ser versionados.

### Resultado consolidado da execucao fisica

- somente a fatia `B1` foi concluida; a Fase B inteira ainda nao terminou;
- duracao controlada total: `91 s`;
- amostras gravadas: `5.461`;
- todos os 9 datasets selecionados ficaram completos e sem warnings;
- cada dataset contem exatamente uma sessao;
- taxas observadas proximas de `60 Hz`;
- `recorderDroppedSamples = 0` em todos os datasets;
- houve apenas duas lacunas isoladas de sequencia:
  - uma em `roll_right`;
  - uma em `roll_left`;
- os outros sete datasets nao tiveram lacunas;
- a recepcao ocorreu em rajadas com interarrival variavel, sem perdas significativas;
- essas metricas temporais nao representam latencia absoluta ponta a ponta.

### Capturas fisicas selecionadas

| Cenario | Dataset | Resultado resumido |
|---|---|---|
| `stationary` | `20260902T221420Z-stationary.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |
| `roll_right` | `20260902T221919Z-roll_right.ndjson` | completo, 1 sessao, sem warnings, 1 lacuna isolada |
| `roll_left` | `20260902T221937Z-roll_left.ndjson` | completo, 1 sessao, sem warnings, 1 lacuna isolada |
| `pitch_front_down` | `20260902T222109Z-pitch_front_down.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |
| `pitch_front_up` | `20260902T222126Z-pitch_front_up.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |
| `yaw_clockwise` | `20260902T222326Z-yaw_clockwise.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |
| `yaw_counterclockwise` | `20260902T222358Z-yaw_counterclockwise.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |
| `linear_forward` | `20260902T222523Z-linear_forward.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |
| `linear_backward` | `20260902T222735Z-linear_backward.ndjson` | completo, 1 sessao, sem warnings, sem lacunas |

### Sinais observados na validacao fisica

- `stationary`:
  - `gravityMps2.z` medio de `-9.812 m/s²`;
  - magnitude media da gravidade de `9.860 m/s²`;
  - medias de aceleracao linear e velocidade angular proximas de zero.
- `roll_right`:
  - `gravityMps2.x` chegou a `+8.552 m/s²`.
- `roll_left`:
  - `gravityMps2.x` chegou a `-9.697 m/s²`.
- `pitch_front_down`:
  - `gravityMps2.y` chegou a `+7.519 m/s²`.
- `pitch_front_up`:
  - `gravityMps2.y` chegou a `-9.290 m/s²`.
- `yaw_clockwise`:
  - primeiro movimento significativo em `angularVelocityRadS.z` negativo.
- `yaw_counterclockwise`:
  - primeiro movimento significativo em `angularVelocityRadS.z` positivo.
- `linear_forward`:
  - aceleracao principal inicial em `linearAccelerationMps2.y` positiva, chegando a `+1.687 m/s²`.
- `linear_backward`:
  - aceleracao principal inicial em `linearAccelerationMps2.y` negativa, chegando a `-1.578 m/s²`.

Os sinais opostos posteriores nas capturas de yaw e deslocamento correspondem a frenagem e retorno a posicao inicial.

### Interpretacao da B1

Esses resultados sustentam a hipotese operacional atual da convencao de montagem:

- `X+ = direita`
- `Y+ = frente`
- `Z+ = cima`

Isso nao constitui um referencial calibrado. A B1 confirmou empiricamente a hipotese de sinais e eixos sob a montagem assumida, mas calibracao/zero, filtros, fusao de sensores, correcao de movimento e overlay continuam fora do escopo desta fatia.

Regras gerais:

- nao dirigir durante os testes;
- nao operar o telefone em veiculo em movimento;
- apoiar o telefone com firmeza;
- manter a mesma convencao de montagem em todos os cenarios;
- registrar observacoes como hipotese, nao como confirmacao.

### 1. `stationary`

- Duracao recomendada: `15 s`
- Posicao inicial: telefone plano e imovel
- Movimento esperado: nenhum
- Hipotese dominante: `gravityMps2.z` proximo de `-9.8`
- Comando de gravacao: `pnpm motion:record -- --scenario stationary --duration-seconds 15`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: anote se `gravity.z` ficou estavel e se houve ruido visivel nos outros eixos

### 2. `roll_right`

- Duracao recomendada: `10 s`
- Posicao inicial: mesmo repouso do cenario anterior
- Movimento esperado: inclinacao controlada para a direita
- Hipotese dominante: componente de gravidade com aumento em `+x`
- Comando de gravacao: `pnpm motion:record -- --scenario roll_right --duration-seconds 10`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: registre se o sinal dominante pareceu coerente, sem afirmar confirmacao definitiva

### 3. `roll_left`

- Duracao recomendada: `10 s`
- Posicao inicial: telefone plano
- Movimento esperado: inclinacao controlada para a esquerda
- Hipotese dominante: componente de gravidade com aumento em `-x`
- Comando de gravacao: `pnpm motion:record -- --scenario roll_left --duration-seconds 10`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: compare com `roll_right` como hipotese espelhada

### 4. `pitch_front_down`

- Duracao recomendada: `10 s`
- Posicao inicial: telefone plano
- Movimento esperado: borda superior inclinada para baixo
- Hipotese dominante: componente de gravidade com aumento em `+y`
- Comando de gravacao: `pnpm motion:record -- --scenario pitch_front_down --duration-seconds 10`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: anote o sinal observado e a repetibilidade do gesto

### 5. `pitch_front_up`

- Duracao recomendada: `10 s`
- Posicao inicial: telefone plano
- Movimento esperado: borda superior inclinada para cima
- Hipotese dominante: componente de gravidade com aumento em `-y`
- Comando de gravacao: `pnpm motion:record -- --scenario pitch_front_up --duration-seconds 10`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: compare com `pitch_front_down` sem tratar a comparacao como prova final

### 6. `yaw_clockwise`

- Duracao recomendada: `10 s`
- Posicao inicial: telefone plano
- Movimento esperado: rotacao horizontal no sentido horario
- Hipotese dominante: `angularVelocityRadS.z` com um sinal consistente
- Comando de gravacao: `pnpm motion:record -- --scenario yaw_clockwise --duration-seconds 10`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: registrar o sinal observado e se a gravidade permaneceu aproximadamente estavel em magnitude

### 7. `yaw_counterclockwise`

- Duracao recomendada: `10 s`
- Posicao inicial: telefone plano
- Movimento esperado: rotacao horizontal no sentido anti-horario
- Hipotese dominante: `angularVelocityRadS.z` com sinal oposto ao de `yaw_clockwise`
- Comando de gravacao: `pnpm motion:record -- --scenario yaw_counterclockwise --duration-seconds 10`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: comparar o sinal com o cenario horario como hipotese

### 8. `linear_forward`

- Duracao recomendada: `8 s`
- Posicao inicial: telefone plano e firmemente apoiado
- Movimento esperado: deslocamento manual curto e seguro para a frente
- Hipotese dominante: `linearAccelerationMps2.y` positivo durante a aceleracao principal
- Comando de gravacao: `pnpm motion:record -- --scenario linear_forward --duration-seconds 8`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: anote se o gesto produziu pico claro ou ambiguidade por tremor manual

### 9. `linear_backward`

- Duracao recomendada: `8 s`
- Posicao inicial: telefone plano e firme
- Movimento esperado: deslocamento manual curto e seguro para tras
- Hipotese dominante: `linearAccelerationMps2.y` negativo durante a aceleracao principal
- Comando de gravacao: `pnpm motion:record -- --scenario linear_backward --duration-seconds 8`
- Comando de analise: `pnpm motion:analyze -- <arquivo.ndjson>`
- Observacoes: compare com `linear_forward` como hipotese oposta, nao como confirmacao
