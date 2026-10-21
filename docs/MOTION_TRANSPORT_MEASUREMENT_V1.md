# Motion Transport Measurement V1 — B5a

## Objetivo e escopo

B5a mede, em uma janela headless controlada, a integridade e a qualidade temporal do stream Android → UDP/Wi-Fi → receiver Rust. O ensaio preserva o dataset NDJSON B1 e produz um relatório analítico JSON estrito `transportMeasurementReportVersion = 1`.

B5a não mede latência de rede, latência ponta a ponta, sensor → tela, renderização ou resposta visual completa. B5b permanece fora de escopo: não há echo, ping/pong, RTT, sincronização de relógios nem alteração do protocolo.

## Fluxo

```text
janela controlada
→ receiver Rust existente
→ recorder B1 com fila bounded e publicação não bloqueante
→ dataset NDJSON v1
→ análise estatística B1 compartilhada
→ relatório B5a JSON v1
→ gates de integridade + baseline exploratória
```

O desktop Tauri deve estar fechado, pois ele e a medição headless usam por padrão `0.0.0.0:57421`. Falha de bind é reportada de forma controlada.

## Relógios e definições

`sessionElapsedUs` é monotônico e relativo à sessão Android. `receivedElapsedUs` é monotônico e relativo ao início local da gravação no desktop. Suas épocas são independentes e nunca são subtraídas diretamente.

Para pares consecutivos dentro da mesma sessão:

- intervalo de origem: `sourceIntervalUs = source[i] - source[i-1]`;
- interarrival de recepção: `receiveInterarrivalUs = receive[i] - receive[i-1]`;
- variação relativa de atraso: `receiveInterarrivalUs - sourceIntervalUs`, preservando sinal;
- variação absoluta relativa de atraso: `abs(receiveInterarrivalUs - sourceIntervalUs)`.

A última pode ser tratada como proxy de jitter, nunca como jitter absoluto ou latência. Cada distribuição reporta `count`, mínimo, média, desvio-padrão populacional, percentis R7 `p50/p95/p99` e máximo, em microssegundos. O desvio populacional divide por `n`. Sem intervalos, a distribuição é `unavailable` com motivo, não zero.

Taxas médias reutilizam a definição B1:

```text
averageRateHz = intervalCount * 1_000_000 / sum(intervalUs)
```

## Gates automáticos de integridade

O `passed` global é derivado, na ordem estável do relatório, somente destes gates:

1. duração observada mínima `requestedDurationUs - 250_000 us`; ultrapassagem pequena não reprova;
2. exatamente uma sessão e zero transições (o início da primeira não é reset);
3. taxa de origem finita, positiva e dentro de `±5%` da taxa declarada de `60 Hz`;
4. taxa de recepção finita, positiva e dentro de `±5%` de `sourceRate * (1 - sequenceLossFraction)`;
5. perda por sequência no máximo `0,1%`, com denominador `observedSamples + estimatedMissingSamples`; sem denominador, a perda é `unavailable` e os gates de perda e coerência de taxa reprovam sem assumir perda zero;
6. zero oversized, inválidos, duplicados/fora de ordem, sessão estrangeira e rate limited nos deltas da janela;
7. zero descartes do recorder, contagem aceita coerente entre receiver e recorder e conclusão limpa.

A tolerância de coerência de taxa é deliberadamente ampla e verifica integridade da captura, não baixa latência. Ela não cria um segundo threshold de perda.

O comando headless não atravessa B4b. `runtimeMetrics` é serializado como `not_applicable`, com motivo `headless_measurement_does_not_traverse_live_tilt_runtime`, e não afeta os gates.

`sequenceLoss` também explicita disponibilidade. Com denominador positivo, usa `availability: "available"` e inclui amostras observadas, ausentes estimadas, denominador e fração. Sem amostras observadas ou estimadas, usa `availability: "unavailable"` com motivo `no_observed_or_estimated_samples`, sem campo `fraction` artificial.

## Baseline exploratória

Percentis, máximo e desvio-padrão não possuem gate na primeira execução física. O relatório os agrupa como baseline exploratória. Como contexto histórico, capturas saudáveis anteriores observaram aproximadamente:

- interarrival `p95`: `33–34 ms`, `p99`: `41 ms`;
- variação relativa assinada `p95`: `14–16 ms`, `p99`: `24 ms`.

Esses valores não são thresholds.

## Validação física aprovada — 21 de outubro de 2026

B5a foi validada fisicamente em Windows com Android standalone, Wi-Fi/LAN local e receiver headless Rust. O relatório local aprovado é `artifacts/motion-measurements/20261021T151839Z-stationary-b5a-v1.json`, derivado do dataset local `artifacts/motion-datasets/20261021T151839Z-stationary.ndjson`; ambos permanecem ignorados pelo Git.

Resumo da execução:

- janela solicitada: `60.000.000 us`;
- duração observada entre primeira e última amostra: `59.995.648 us`;
- `3.601` amostras, sequências `0..3600`;
- uma sessão, zero transições, zero gaps e zero amostras ausentes estimadas;
- perda por sequência: `0%`;
- taxa média de origem: `59,9964742072 Hz`;
- taxa média recebida: `60,0043523157 Hz`;
- diferença absoluta: `0,0078781085 Hz`;
- diferença relativa: aproximadamente `0,01313%`;
- todos os 14 gates de integridade aprovados.

No período armado houve zero datagramas e zero rejeições. Durante a janela, o receiver registrou `3.601` datagramas recebidos e `3.601` amostras aceitas, com zero oversized, inválidos, duplicados/fora de ordem, sessão estrangeira e rate limited. O recorder recebeu `3.601` eventos aceitos, escreveu `3.601` amostras, teve zero descartes e conclusão limpa. O runtime B4b foi `not_applicable`, com motivo `headless_measurement_does_not_traverse_live_tilt_runtime`.

### Primeira baseline exploratória formal

As distribuições abaixo têm `3.600` intervalos cada e não participam dos gates:

| Distribuição | Média | Desvio-padrão populacional | Mínimo | p50 | p95 | p99 | Máximo |
|---|---:|---:|---:|---:|---:|---:|---:|
| Intervalo de origem | `16.667,646 us` | `4.657,093 us` | `9.947 us` | `19.895 us` | `19.896 us` | `19.896 us` | `19.896 us` |
| Interarrival de recepção | `16.665,458 us` | `8.547,669 us` | `44 us` | `19.143 us` | `31.715,65 us` | `38.971,63 us` | `44.609 us` |
| Variação relativa assinada de atraso | `-2,188 us` | `7.143,793 us` | `-19.851 us` | `333,5 us` | `14.548,2 us` | `19.878,16 us` | `26.331 us` |
| Variação absoluta relativa de atraso / proxy de jitter | `4.462,188 us` | `5.578,768 us` | `0 us` | `1.745,5 us` | `17.969,45 us` | `19.878,16 us` | `26.331 us` |

Em comparação meramente descritiva, o interarrival ficou coerente ou ligeiramente abaixo dos valores históricos (`p95` aproximadamente `33–34 ms`, `p99` aproximadamente `41 ms`), assim como a variação relativa assinada de atraso (`p95` aproximadamente `14–16 ms`, `p99` aproximadamente `24 ms`). Esses dados são baseline exploratória, não thresholds nem conclusão de desempenho. B5a não mediu latência de rede, latência ponta a ponta ou resposta sensor → tela; a variação absoluta relativa de atraso é somente um proxy de jitter, não jitter absoluto.

### Evidência adicional de lifecycle

Uma tentativa anterior sem stream foi interrompida com Ctrl+C antes da primeira amostra. O receiver e o recorder encerraram, o dataset vazio foi removido, nenhum relatório foi produzido e a execução retornou código operacional `1`. Foi confirmada a ausência de:

- `artifacts/motion-datasets/20261021T151800Z-stationary.ndjson`;
- `artifacts/motion-measurements/20261021T151800Z-stationary-b5a-v1.json`.

Esse resultado confirma o comportamento operacional seguro e não integra a medição física B5a aprovada. Com esta validação, B5a está concluída. B5b não foi implementada e permanece separada; a Fase B continua aberta exclusivamente por B5b, e a Fase C ainda não começou.

## Operação e arquivos

Ensaio padrão de 60 segundos:

```powershell
pnpm motion:measure -- --duration-seconds 60
```

Opções:

```text
--duration-seconds <inteiro positivo>
--startup-timeout-seconds <inteiro positivo>
--bind <host:port>
--dataset-output <arquivo.ndjson>
--report-output <arquivo.json>
```

Os padrões ficam em `artifacts/motion-datasets/` e `artifacts/motion-measurements/`; ambos são ignorados pelo Git. Arquivos existentes nunca são sobrescritos silenciosamente. Caminhos e horário operacional ficam fora da parte analítica determinística.

O receiver e o recorder iniciam primeiro e o comando mostra que está aguardando o stream. A janela nominal começa somente quando chega a primeira amostra aceita; essa primeira amostra já pertence ao dataset. A espera não consome os 60 segundos da medição. Por padrão, `--startup-timeout-seconds` é `60`. Se nenhuma amostra for aceita nesse prazo, receiver e recorder são encerrados, o dataset vazio é removido, nenhum relatório B5a é emitido e a execução retorna erro operacional (`1`).

O baseline dos contadores é capturado atomicamente pelo `ReceiverState` no momento da primeira aceitação. `armingReceiverCounters` registra o tráfego anterior à primeira amostra e é somente diagnóstico. `receiverCounters` é o delta entre esse baseline e o snapshot obtido após encerrar o receiver; ele inclui o datagrama e a aceitação da primeira amostra, cobre somente a janela medida e é a única seção de contadores usada pelos gates. Assim, tráfego inválido durante o período armado permanece visível, mas não reprova a janela.

Saídas e códigos:

- `0`: medição concluída e gates aprovados;
- `2`: medição concluída, relatório escrito e um ou mais gates reprovados;
- `1`: erro de execução/argumento ou dados insuficientes para intervalos.

Ctrl+C antes da primeira amostra encerra receiver e recorder, remove o dataset vazio e não emite relatório. Ctrl+C durante a janela escreve summary interrompido quando há dados; o relatório ainda é emitido quando existem intervalos analisáveis e os gates de duração/conclusão reprovam. Sem intervalos suficientes, o dataset é preservado e a execução retorna erro controlado.

## Determinismo e limitações

Para o mesmo NDJSON, contadores da janela e configuração, cálculos, percentis, gates, ordenação e JSON analítico são idênticos. Timestamp atual, caminhos absolutos e tamanho do arquivo não entram no relatório analítico.

O receiver registra apenas amostras aceitas; rejeições aparecem nos deltas de contadores. Gaps estimam ausência por sequência sem fabricar amostras e sem tratar troca de sessão como gap comum.

## Checklist físico

- fechar o desktop Tauri;
- apoiar o Android na convenção `flat_screen_up_portrait_top_toward_vehicle_front`;
- confirmar destino IPv4/porta no Android;
- iniciar `motion:measure`;
- aguardar a mensagem `receiver ready; waiting for the first accepted sample`;
- iniciar o stream no Android;
- evitar trocar/reiniciar a sessão durante a janela;
- aguardar encerramento automático;
- preservar NDJSON e JSON locais;
- revisar gates e baseline exploratória;
- registrar a validação física separadamente — a implementação, por si só, não valida B5a fisicamente. O checklist foi concluído para a execução aprovada de 21 de outubro de 2026 descrita acima.
