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

import org.junit.jupiter.api.Test;

import java.security.cert.X509Certificate;
import java.util.Date;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SelfSignedCertificateTest {

    @Test
    void createDefaultCertificate() {
        try (SelfSignedCertificate cert = SelfSignedCertificate.create()) {
            assertNotNull(cert.privateKey(), "Private key must not be null");
            assertNotNull(cert.certificate(), "Certificate must not be null");
            assertNotNull(cert.certificateChain(), "Certificate chain must not be null");
            assertEquals(1, cert.certificateChain().length);
        }
    }

    @Test
    void createCertificateWithCustomCn() {
        try (SelfSignedCertificate cert = SelfSignedCertificate.create("myhost.example.com")) {
            X509Certificate x509 = cert.certificate();
            String subjectDN = x509.getSubjectX500Principal().getName();
            assertTrue(subjectDN.contains("myhost.example.com"),
                    "Subject DN should contain the specified CN. Got: " + subjectDN);
        }
    }

    @Test
    void certificateIsCurrentlyValid() {
        try (SelfSignedCertificate cert = SelfSignedCertificate.create()) {
            assertDoesNotThrow(() -> cert.certificate().checkValidity(new Date()),
                    "Certificate should be valid at the current time");
        }
    }

    @Test
    void privateKeyIsEcKey() {
        try (SelfSignedCertificate cert = SelfSignedCertificate.create()) {
            assertEquals("EC", cert.privateKey().getAlgorithm(),
                    "Private key should be an EC key for fast generation");
        }
    }
}
