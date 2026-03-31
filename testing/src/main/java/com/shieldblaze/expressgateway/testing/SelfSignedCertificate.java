/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.testing;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.asn1.x509.Extension;
import org.bouncycastle.asn1.x509.GeneralName;
import org.bouncycastle.asn1.x509.GeneralNames;
import org.bouncycastle.cert.X509CertificateHolder;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.bouncycastle.operator.ContentSigner;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;

import java.io.Closeable;
import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.Security;
import java.security.cert.X509Certificate;
import java.security.spec.ECGenParameterSpec;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.Date;

/**
 * Generates a self-signed X.509 certificate and key pair for test use.
 * Uses EC P-256 for fast key generation in tests (RSA is slower).
 *
 * <p>Example:</p>
 * <pre>
 *   try (SelfSignedCertificate cert = SelfSignedCertificate.create("localhost")) {
 *       SslContext ctx = SslContextBuilder.forServer(cert.privateKey(), cert.certificate()).build();
 *   }
 * </pre>
 */
public final class SelfSignedCertificate implements Closeable {

    static {
        if (Security.getProvider(BouncyCastleProvider.PROVIDER_NAME) == null) {
            Security.addProvider(new BouncyCastleProvider());
        }
    }

    private final PrivateKey privateKey;
    private final X509Certificate certificate;

    private SelfSignedCertificate(PrivateKey privateKey, X509Certificate certificate) {
        this.privateKey = privateKey;
        this.certificate = certificate;
    }

    /**
     * Create a self-signed certificate for the given common name.
     *
     * @param cn the common name (typically "localhost" for tests)
     * @return a new SelfSignedCertificate
     */
    public static SelfSignedCertificate create(String cn) {
        try {
            KeyPairGenerator keyPairGen = KeyPairGenerator.getInstance("EC", BouncyCastleProvider.PROVIDER_NAME);
            keyPairGen.initialize(new ECGenParameterSpec("secp256r1"));
            KeyPair keyPair = keyPairGen.generateKeyPair();

            Instant now = Instant.now();
            X500Name issuer = new X500Name("CN=" + cn);

            JcaX509v3CertificateBuilder certBuilder = new JcaX509v3CertificateBuilder(
                    issuer,
                    BigInteger.valueOf(now.toEpochMilli()),
                    Date.from(now),
                    Date.from(now.plus(365, ChronoUnit.DAYS)),
                    issuer,
                    keyPair.getPublic()
            );

            // Add Subject Alternative Names (SAN) so JDK's HttpClient
            // hostname verification passes for 127.0.0.1 and localhost.
            GeneralNames sans = new GeneralNames(new GeneralName[]{
                    new GeneralName(GeneralName.dNSName, cn),
                    new GeneralName(GeneralName.dNSName, "localhost"),
                    new GeneralName(GeneralName.iPAddress, "127.0.0.1")
            });
            certBuilder.addExtension(Extension.subjectAlternativeName, false, sans);

            ContentSigner signer = new JcaContentSignerBuilder("SHA256withECDSA")
                    .setProvider(BouncyCastleProvider.PROVIDER_NAME)
                    .build(keyPair.getPrivate());

            X509CertificateHolder holder = certBuilder.build(signer);
            X509Certificate cert = new JcaX509CertificateConverter()
                    .setProvider(BouncyCastleProvider.PROVIDER_NAME)
                    .getCertificate(holder);

            return new SelfSignedCertificate(keyPair.getPrivate(), cert);
        } catch (Exception ex) {
            throw new RuntimeException("Failed to generate self-signed certificate", ex);
        }
    }

    /**
     * Create a self-signed certificate for "localhost".
     *
     * @return a new SelfSignedCertificate
     */
    public static SelfSignedCertificate create() {
        return create("localhost");
    }

    public PrivateKey privateKey() {
        return privateKey;
    }

    public X509Certificate certificate() {
        return certificate;
    }

    public X509Certificate[] certificateChain() {
        return new X509Certificate[]{certificate};
    }

    @Override
    public void close() {
        // No filesystem resources to clean up; certificate and key are in-memory only
    }
}
