/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.common.utils;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.asn1.x500.X500NameBuilder;
import org.bouncycastle.asn1.x500.style.BCStyle;
import org.bouncycastle.asn1.x509.BasicConstraints;
import org.bouncycastle.asn1.x509.Extension;
import org.bouncycastle.asn1.x509.GeneralName;
import org.bouncycastle.asn1.x509.GeneralNamesBuilder;
import org.bouncycastle.asn1.x509.KeyUsage;
import org.bouncycastle.cert.X509CertificateHolder;
import org.bouncycastle.cert.X509v3CertificateBuilder;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509ExtensionUtils;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.bouncycastle.operator.ContentSigner;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;

import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.Provider;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;
import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.util.Date;
import java.util.List;

/**
 * This class generates Self Signed Certificate using ECC-256
 * with all IP addresses and DNS-names in SAN (Subject Alternative Name).
 *
 * <p></p>
 * <p>
 * Use {@link #generateNew(List)} method to create a new Instance of
 * {@link SelfSignedCertificate}.
 */
public final class SelfSignedCertificate {

    private static final Logger logger = LogManager.getLogger(SelfSignedCertificate.class);
    private static final Provider PROVIDER = new BouncyCastleProvider();

    private final X509Certificate x509Certificate;
    private final KeyPair keyPair;

    /**
     * Create a new {@link SelfSignedCertificate} Instance
     *
     * @param x509Certificate {@link X509Certificate} instance of generated certificate
     * @param keyPair         {@link KeyPair} instance of generated keypair
     */
    private SelfSignedCertificate(X509Certificate x509Certificate, KeyPair keyPair) {
        this.x509Certificate = x509Certificate;
        this.keyPair = keyPair;
    }

    public X509Certificate x509Certificate() {
        return x509Certificate;
    }

    public KeyPair keyPair() {
        return keyPair;
    }

    /**
     * Generate new {@link SelfSignedCertificate} instance
     *
     * @param ipList  List of IP addresses in SAN
     * @return {@link SelfSignedCertificate} instance once successful
     */
    public static SelfSignedCertificate generateNew(List<String> ipList) {
        try {
            JcaX509ExtensionUtils extUtils = new JcaX509ExtensionUtils();

            KeyPairGenerator keyGen = KeyPairGenerator.getInstance("EC");
            keyGen.initialize(256);
            KeyPair keyPair = keyGen.generateKeyPair();

            X500Name x500Name = new X500NameBuilder(BCStyle.INSTANCE)
                    .addRDN(BCStyle.O, "ShieldBlaze")
                    .addRDN(BCStyle.OU, "ShieldBlaze ExpressGateway")
                    .addRDN(BCStyle.CN, "ExpressGateway REST-API Self-Signed Certificate")
                    .build();

            X509v3CertificateBuilder certificateHolder = new JcaX509v3CertificateBuilder(
                    x500Name,
                    new BigInteger(160, new SecureRandom()),
                    Date.from(ZonedDateTime.now().minusDays(3).withZoneSameInstant(ZoneOffset.UTC).toInstant()),
                    Date.from(ZonedDateTime.now().plusYears(10).withZoneSameInstant(ZoneOffset.UTC).toInstant()),
                    x500Name,
                    keyPair.getPublic()
            );

            certificateHolder.addExtension(Extension.subjectKeyIdentifier, false, extUtils.createSubjectKeyIdentifier(keyPair.getPublic()));
            certificateHolder.addExtension(Extension.authorityKeyIdentifier, false, extUtils.createAuthorityKeyIdentifier(keyPair.getPublic()));

            BasicConstraints constraints = new BasicConstraints(false);
            certificateHolder.addExtension(Extension.basicConstraints, true, constraints.getEncoded());

            KeyUsage keyUsage = new KeyUsage(KeyUsage.digitalSignature);
            certificateHolder.addExtension(Extension.keyUsage, true, keyUsage.getEncoded());

            // SAN
            GeneralNamesBuilder generalNamesBuilder = new GeneralNamesBuilder();
            for (String ip : ipList) {
                generalNamesBuilder.addName(new GeneralName(GeneralName.iPAddress, ip));
            }
            certificateHolder.addExtension(Extension.subjectAlternativeName, false, generalNamesBuilder.build());

            ContentSigner signer = new JcaContentSignerBuilder("SHA256withECDSA").build(keyPair.getPrivate());
            X509CertificateHolder certHolder = certificateHolder.build(signer);
            X509Certificate cert = new JcaX509CertificateConverter()
                    .setProvider(PROVIDER)
                    .getCertificate(certHolder);

            return new SelfSignedCertificate(cert, keyPair);
        } catch (Exception ex) {
            logger.error("Failed to generate SelfSignedCertificate | Shutting down...", ex);
            System.exit(1);
            return null; // We should never reach here.
        }
    }

    @Override
    public String toString() {
        return "SelfSignedCertificate{x509Certificate=" + x509Certificate + '}';
    }
}
