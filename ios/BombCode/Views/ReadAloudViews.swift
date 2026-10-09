import SwiftUI

/// The read-aloud mini player, pinned above the composer while a reply in this thread is read.
struct MiniPlayer: View {
    private let reader = ReadAloud.shared
    /// While dragging: where the thumb is (seconds); the timeline follows it, not the audio.
    @State private var scrubbing: Double?

    var body: some View {
        let status = reader.status
        let total = max(status.estimatedTotal, 0.001)
        let position = scrubbing ?? status.position
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: "waveform")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                    .symbolEffect(.variableColor.iterative, isActive: status.playing)
                Button { reader.reveal = reader.playing?.entry } label: {
                    Text(status.failed ?? reader.playing?.title ?? "")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textMuted)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
                .accessibilityHint("Shows the message being read")
                Menu {
                    ForEach(ReadAloud.rates, id: \.self) { rate in
                        Button { ReadAloud.shared.rate = rate } label: {
                            if rate == reader.rate { Label(speed(rate), systemImage: "checkmark") } else { Text(speed(rate)) }
                        }
                    }
                } label: {
                    Text(speed(reader.rate))
                        .font(Theme.mono(12, .medium))
                        .foregroundStyle(Theme.text)
                        .padding(.horizontal, 8)
                        .frame(minHeight: 26)
                        .overlay(Capsule().stroke(Theme.pillBorder, lineWidth: 1))
                }
                .accessibilityLabel("Speed")
                Button { reader.close() } label: {
                    Image(systemName: "xmark").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.textMuted).frame(width: 26, height: 26)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop reading")
            }
            HStack(spacing: 10) {
                Button { reader.skip(-15) } label: { Image(systemName: "gobackward.15").font(.system(size: 17)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text).accessibilityLabel("Back 15 seconds")
                Button { reader.playPause() } label: {
                    Image(systemName: status.playing ? "pause.fill" : "play.fill")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundStyle(Theme.onSolid)
                        .frame(width: 34, height: 34)
                        .background(Circle().fill(Theme.solid))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(status.playing ? "Pause" : "Play")
                Button { reader.skip(15) } label: { Image(systemName: "goforward.15").font(.system(size: 17)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text).accessibilityLabel("Forward 15 seconds")
                Text(clock(position)).font(Theme.mono(11)).foregroundStyle(Theme.textFaint).monospacedDigit()
                Timeline(position: position, rendered: status.duration, total: total) { seconds, done in
                    scrubbing = done ? nil : seconds
                    if done { reader.seek(to: seconds) }
                }
                Text((status.complete || status.total == 0 ? "" : "~") + clock(total)).font(Theme.mono(11)).foregroundStyle(Theme.textFaint).monospacedDigit()
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: Theme.panelCorner))
        .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
    }

    private func speed(_ rate: Float) -> String {
        rate == rate.rounded() ? "\(Int(rate))×" : "\(rate)×"
    }
}

/// The timeline: what's played, what's rendered (and what isn't yet), draggable to seek.
private struct Timeline: View {
    let position: Double
    let rendered: Double
    let total: Double
    let seek: (Double, Bool) -> Void

    var body: some View {
        GeometryReader { geo in
            let width = geo.size.width
            let played = CGFloat(min(max(position / total, 0), 1)) * width
            let ready = CGFloat(min(max(rendered / total, 0), 1)) * width
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.bubble).frame(height: 4)
                Capsule().fill(Theme.textFaint.opacity(0.35)).frame(width: ready, height: 4)
                Capsule().fill(Theme.accent).frame(width: played, height: 4)
                Circle().fill(Theme.accent).frame(width: 12, height: 12).offset(x: played - 6)
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0)
                .onChanged { seek(Double(min(max($0.location.x / width, 0), 1)) * total, false) }
                .onEnded { seek(Double(min(max($0.location.x / width, 0), 1)) * total, true) })
        }
        .frame(height: 28)
        .accessibilityElement()
        .accessibilityLabel("Position")
        .accessibilityValue("\(clock(position)) of \(clock(total))")
        .accessibilityAdjustableAction { direction in
            seek(min(max(position + (direction == .increment ? 15 : -15), 0), total), true)
        }
    }
}

private func clock(_ seconds: Double) -> String {
    let s = Int(max(seconds, 0))
    return "\(s / 60):" + String(format: "%02d", s % 60)
}

/// Settings → Voice: the Fish Audio key, then the voices (Recommended / Popular / Mine) with previews.
struct VoiceSettings: View {
    @Environment(\.dismiss) private var dismiss
    private let reader = ReadAloud.shared
    @State private var keyText = ""
    @State private var searchText = ""

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(title: "Voice") { EmptyView() } trailing: {
                Button("Done") { reader.stopPreview(); dismiss() }.foregroundStyle(Theme.text)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    Footnote("Replies are read with Fish Audio voices, using your own Fish Audio API key. Tap the speaker under a reply.")
                    keyCard
                    SectionLabel("Reading with \(reader.chosenVoiceName)").padding(.top, 6)
                    Picker("Voices", selection: Binding(get: { reader.voiceList }, set: { reader.showList($0) })) {
                        Text("Recommended").tag("recommended")
                        Text("Popular").tag("popular")
                        Text("Mine").tag("mine")
                    }
                    .pickerStyle(.segmented)
                    HStack(spacing: 8) {
                        Image(systemName: "magnifyingglass").foregroundStyle(Theme.textFaint)
                        TextField("Search voices", text: $searchText)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .foregroundStyle(Theme.text)
                    }
                    .padding(.horizontal, 12)
                    .frame(height: 38)
                    .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
                    .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
                    list
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
                .frame(maxWidth: .infinity)
            }
            .scrollIndicators(.hidden)
            // Up and down only: nothing here is wider than the screen.
            .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
            .scrollDismissesKeyboard(.interactively)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.5) }
        .clipped()
        .presentationDragIndicator(.visible)
        .onChange(of: searchText) { _, text in reader.searchVoices(text) }
        .onAppear {
            searchText = reader.query
            if reader.voices.isEmpty { reader.refreshVoices() }
            #if DEBUG
            if Smoke.enabled { print("smoke: voice sheet shown") }
            #endif
        }
        .onDisappear { reader.stopPreview() }
    }

    private var keyCard: some View {
        GlassCard {
            Text("Fish Audio API key").font(Theme.sans(15, .semibold)).foregroundStyle(Theme.text)
            if reader.hasKey {
                HStack {
                    Text("Saved on this iPhone").font(Theme.small).foregroundStyle(Theme.textMuted)
                    Spacer()
                    Button("Remove", role: .destructive) { reader.removeKey() }.font(Theme.small)
                }
            } else {
                SecureField("Paste your API key", text: $keyText)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .foregroundStyle(Theme.text)
                    .padding(.horizontal, 10)
                    .frame(height: 36)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Theme.bubble))
                if let error = reader.keyError {
                    Text(error).font(Theme.small).foregroundStyle(Theme.danger).fixedSize(horizontal: false, vertical: true)
                }
                HStack {
                    Link("Get a key at fish.audio", destination: URL(string: "https://fish.audio/app/api-keys/")!).font(Theme.small)
                    Spacer()
                    if reader.checkingKey {
                        ProgressView().controlSize(.small).tint(Theme.textMuted)
                        Text("Checking…").font(Theme.small).foregroundStyle(Theme.textMuted)
                    } else {
                        Button("Save") { Task { if await reader.saveKey(keyText) { keyText = "" } } }
                            .font(Theme.sans(14, .semibold))
                            .disabled(keyText.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            }
            Text("Uses Fish Audio's free s2.1-pro-free model.").font(Theme.caption).foregroundStyle(Theme.textFaint)
        }
    }

    @ViewBuilder
    private var list: some View {
        if let error = reader.voicesError {
            Text(error).font(Theme.small).foregroundStyle(Theme.danger).fixedSize(horizontal: false, vertical: true)
        } else if reader.voicesLoading && reader.voices.isEmpty {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small).tint(Theme.textMuted)
                Text("Looking for voices…").font(Theme.small).foregroundStyle(Theme.textMuted)
            }
            .padding(.top, 8)
        } else if reader.voices.isEmpty {
            Footnote(reader.voiceList == "mine" ? "No voices of your own yet. Clone one at fish.audio and it shows up here." : "No voices match.")
        }
        let chosen = reader.chosenVoice
        if !reader.voices.isEmpty {
            LazyVStack(spacing: 0) {
                ForEach(Array(reader.voices.enumerated()), id: \.element.id) { index, voice in
                    if index > 0 { Rectangle().fill(Theme.hairline).frame(height: 1).padding(.leading, 40) }
                    VoiceRow(voice: voice, chosen: chosen == voice.id)
                }
            }
            .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
            .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
        }
    }
}

private struct VoiceRow: View {
    let voice: SpeechVoiceInfo
    let chosen: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.accent).opacity(chosen ? 1 : 0).frame(width: 16)
            VStack(alignment: .leading, spacing: 2) {
                Text(voice.name).font(Theme.sans(15, chosen ? .medium : .regular)).foregroundStyle(Theme.text).lineLimit(1)
                Text(detail).font(Theme.caption).foregroundStyle(Theme.textFaint).lineLimit(1)
            }
            Spacer(minLength: 8)
            let previewing = ReadAloud.shared.previewing == voice.id
            Button { ReadAloud.shared.togglePreview(voice) } label: {
                Image(systemName: previewing ? "pause.circle.fill" : "play.circle")
                    .font(.system(size: 22))
                    .foregroundStyle(previewing ? Theme.accent : Theme.textMuted)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(previewing ? "Stop preview" : "Preview \(voice.name)")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .contentShape(Rectangle())
        .onTapGesture { ReadAloud.shared.choose(voice) }
    }

    private var detail: String {
        let languages = voice.languages.map { Locale.current.localizedString(forLanguageCode: $0) ?? $0 }.joined(separator: ", ")
        return [voice.author, languages].filter { !$0.isEmpty }.joined(separator: " · ")
    }
}
