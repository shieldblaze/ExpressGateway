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
package com.shieldblaze.expressgateway.configuration.tls;

import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Disabled;
import org.junit.jupiter.api.Test;

import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLException;
import java.io.IOException;
import java.net.URL;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

class CertificateKeyPairTest {

    @Test
    void clientECCCertificateKeyTest() throws CertificateException, SSLException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);

        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key());
        assertNull(certificateKeyPair.sslContext());

        // Initialize the CertificateKeyPair
        certificateKeyPair.init(TLSClientConfiguration.DEFAULT);

        assertFalse(certificateKeyPair.useOCSPStapling());
        assertNull(certificateKeyPair.ocspStaplingData());
        assertNotNull(certificateKeyPair.sslContext());
    }

    @Test
    void clientRSACertificateKeyTest() throws CertificateException, SSLException {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "RSA", 2048);

        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key());
        assertNull(certificateKeyPair.sslContext());

        // Initialize the CertificateKeyPair
        certificateKeyPair.init(TLSClientConfiguration.DEFAULT);

        assertFalse(certificateKeyPair.useOCSPStapling());
        assertNull(certificateKeyPair.ocspStaplingData());
        assertNotNull(certificateKeyPair.sslContext());
    }

    @Test
    @Disabled("Need Certificate with it's Private Key to run this test")
    void ocspStaplingTest() throws IOException, CertificateException, InterruptedException {
        OpenSsl.ensureAvailability();

        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);

        URL url = new URL("https://www.shieldblaze.com");
        HttpsURLConnection con = (HttpsURLConnection) url.openConnection();
        con.connect();

        X509Certificate[] certs = (X509Certificate[]) con.getServerCertificates();
        List<X509Certificate> x509Certificates = new ArrayList<>(Arrays.asList(certs));

        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forServer(x509Certificates, selfSignedCertificate.key(), true);
        certificateKeyPair.init(TLSServerConfiguration.DEFAULT);

        Thread.sleep(1000 * 15); // Wait for 15 seconds, Timeout for OCSP HTTP Client Request

        assertNotNull(certificateKeyPair.ocspStaplingData());
    }
}
