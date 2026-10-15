// QR verification (spec §7.7) in the app only: app extensions cannot use the camera, and the
// Share extension does not receive, so it could not answer a scan of its own QR code either.
// - "Verifica con QR": AVFoundation scanner → HoppyNode.qrScan (the core checks the key).
// - "Mostra il mio QR": CoreImage QR code of HoppyNode.qrBegin, closed on `verified`.
import AVFoundation
import CoreImage
import SwiftUI
import UIKit

enum CameraAccess {
    /// Calls back on the main queue: true when the camera may be used.
    static func request(_ done: @escaping (Bool) -> Void) {
        guard AVCaptureDevice.default(for: .video) != nil else { return done(false) }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: done(true)
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { ok in DispatchQueue.main.async { done(ok) } }
        default: done(false)
        }
    }

    static var hasCamera: Bool { AVCaptureDevice.default(for: .video) != nil }
}

enum QRCode {
    /// Black modules on white, scaled without smoothing.
    static func image(_ text: String) -> UIImage? {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 12, y: 12)),
              let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
}

/// "Mostra il mio QR": the other phone scans it inside Hoppy.
struct MyQRSheet: View {
    let payload: String
    let onClose: () -> Void
    @Environment(\.colorScheme) var scheme

    var body: some View {
        let palette = Palette.current(scheme)
        NavigationView {
            ScrollView {
                VStack(spacing: 18) {
                    Text(L("qr_hint"))
                        .font(.body)
                        .foregroundColor(palette.text)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                    if let image = QRCode.image(payload) {
                        Image(uiImage: image)
                            .interpolation(.none)
                            .resizable()
                            .scaledToFit()
                            .frame(maxWidth: 340)
                            .padding(16)
                            .background(Color.white)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                            .accessibilityLabel(L("qr_image_label"))
                    }
                    Text(L("qr_expires"))
                        .font(.footnote)
                        .foregroundColor(palette.muted)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(24)
                .frame(maxWidth: .infinity)
            }
            .background(palette.bg.ignoresSafeArea())
            .navigationTitle(L("qr_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(L("close"), action: onClose) }
            }
        }
        .navigationViewStyle(.stack)
    }
}

/// "Verifica con QR": camera preview, reports the first QR code it reads.
struct QRScanSheet: View {
    let onCode: (String) -> Void
    let onCancel: () -> Void
    @State var failed = false

    var body: some View {
        NavigationView {
            ZStack(alignment: .bottom) {
                Color.black.ignoresSafeArea()
                if failed {
                    Text(L("camera_unavailable"))
                        .foregroundColor(.white)
                        .multilineTextAlignment(.center)
                        .padding(24)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    QRScannerView(onCode: onCode, onFail: { failed = true }).ignoresSafeArea()
                    Text(L("scan_hint"))
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(.white)
                        .multilineTextAlignment(.center)
                        .padding(12)
                        .background(RoundedRectangle(cornerRadius: 12).fill(Color.black.opacity(0.75)))
                        .padding(20)
                }
            }
            .navigationTitle(L("scan_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(L("cancel"), action: onCancel) }
            }
        }
        .navigationViewStyle(.stack)
    }
}

struct QRScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onFail: () -> Void

    func makeUIViewController(context: Context) -> QRScannerController {
        let c = QRScannerController()
        c.onCode = onCode
        c.onFail = onFail
        return c
    }

    func updateUIViewController(_ controller: QRScannerController, context: Context) {}
}

final class QRScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?
    var onFail: (() -> Void)?
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "hoppy.qr.camera")
    private var preview: AVCaptureVideoPreviewLayer?
    private var reported = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return fail() }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return fail() }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        guard output.availableMetadataObjectTypes.contains(.qr) else { return fail() }
        output.metadataObjectTypes = [.qr]
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(layer)
        preview = layer
    }

    private func fail() {
        DispatchQueue.main.async { [weak self] in self?.onFail?() }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
        if let connection = preview?.connection, connection.isVideoOrientationSupported,
           let orientation = view.window?.windowScene?.interfaceOrientation {
            switch orientation {
            case .landscapeLeft: connection.videoOrientation = .landscapeLeft
            case .landscapeRight: connection.videoOrientation = .landscapeRight
            case .portraitUpsideDown: connection.videoOrientation = .portraitUpsideDown
            default: connection.videoOrientation = .portrait
            }
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        let session = self.session
        queue.async { if !session.isRunning, !session.inputs.isEmpty { session.startRunning() } }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let session = self.session
        queue.async { if session.isRunning { session.stopRunning() } }
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject],
                        from connection: AVCaptureConnection) {
        guard !reported,
              let code = objects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first
        else { return }
        reported = true
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        onCode?(code)
    }
}
