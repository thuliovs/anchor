# 🏗️ Anchor: Briefing de Arquitetura e Engenharia

**Status do Projeto:** `Infraestrutura Pronta` | `Monorepo Configurado` **Arquitetura:** Sistema Distribuído de Baixa Latência (Sensor -> Overlay)

### 1\. O Problema de Engenharia

O objetivo é mitigar a cinetose (motion sickness) fornecendo feedback visual de horizonte artificial em tempo real. O desafio técnico central não é visual, é **temporal**. O sistema precisa capturar dados físicos (aceleração/giro) em um dispositivo móvel, serializar, transmitir via rede e renderizar no PC com latência sub-perceptual (< 20ms idealmente). Qualquer atraso ou "jitter" perceptível pode agravar o enjoo em vez de curá-lo.

### 2\. A Stack Tecnológica (E os "Porquês")

Decidimos por uma arquitetura híbrida para maximizar performance onde é crítico e produtividade onde é possível.

#### 🖥️ Desktop: Tauri v2 (Rust + React)

Não escolhemos Electron por questões de consumo de recursos. O PC do usuário estará rodando softwares pesados de trabalho; o Anchor deve ser invisível em termos de CPU/RAM.

-   **Backend (Rust):** O Rust não está lá por acaso. Ele é responsável pela thread de rede (UDP) e parser de dados. Precisamos da garantia de _memory safety_ e da capacidade de processar pacotes em alta frequência sem o _Garbage Collector_ pausar a execução.
    
-   **Frontend (React):** A camada visual. O Tauri nos permite desenhar o horizonte usando tecnologias web (Canvas/CSS/WebGL), facilitando a criação de uma UI moderna e responsiva sem a dor de cabeça de frameworks de GUI nativos (GTK/Win32).
    

#### 📱 Mobile: React Native

Atuará exclusivamente como _publisher_ de dados.

-   **Escolha:** Optamos pelo React Native pela facilidade de acesso à API de sensores nativos e pela velocidade de iteração. O desafio aqui é garantir que a "Bridge" do JS não se torne um gargalo na leitura dos sensores (Gyroscope/Accelerometer) em frequências altas (60Hz+).
    

#### 🔗 Comunicação: UDP (User Datagram Protocol)

-   **Decisão Crítica:** TCP está proibido para o streaming de dados. Não podemos esperar pelo _handshake_ ou retransmissão de pacotes perdidos. Se um pacote de dados de movimento se perdeu, ele já é passado; precisamos do próximo. A comunicação deve ser _fire-and-forget_.
    

### 3\. Estrutura do Monorepo & Contratos

O projeto já está configurado como um Monorepo (`pnpm workspaces`) para resolver o problema de **consistência de tipos**.

-   **`packages/protocol`:** Esta é a "Single Source of Truth". Aqui definimos as interfaces (Structs/Types) do payload. Tanto o Rust quanto o React Native consomem este pacote.
    
    -   _Diretriz:_ Qualquer alteração no formato dos dados (ex: mudar de Euler Angles para Quaternions) deve começar aqui.
        

### 4\. Seus Desafios de Implementação (Abertos)

A infraestrutura é o chão; você vai construir a casa. Existem problemas em aberto que precisam da sua engenharia:

1.  **Overlay & Transparência:** O Frontend do Tauri precisa se comportar como um "Fantasma". Ele deve ficar `Always On Top`, ser transparente (alpha channel) e, crucialmente, ser `Click-through` (ignorar eventos de mouse para que o usuário possa clicar nas janelas atrás do horizonte). Como gerenciar estados onde o usuário _precisa_ clicar no app (configurações) vs quando ele deve ser ignorado?
    
2.  **Matemática Espacial (The Drift Problem):** Giroscópios sofrem de _drift_ (perda de calibração) ao longo do tempo. Acelerômetros são ruidosos.
    
    -   _Desafio:_ Como você vai tratar os dados brutos? Filtro de Kalman? Complementary Filter? Fusão de sensores? A suavidade da animação depende dessa lógica matemática.
        
3.  **Resiliência de Rede:** O que acontece se o pacote atrasar?
    
    -   _Interpolação:_ O PC deve "adivinhar" o movimento entre dois pacotes para manter a animação fluida (60/120fps) mesmo se a rede estiver a 30fps?
        
    -   _Timeout:_ Como a UI reage se o celular desconectar?
        

### Resumo

Você tem um backend Rust performático pronto para ouvir e um app Mobile pronto para ser escrito. O objetivo é conectar essas pontas de forma que o usuário esqueça que existe uma rede entre elas.
