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
package com.shieldblaze.expressgateway.configuration.tls;

import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertThrows;

class OCSPClientTest {

    @Test
    void fetchEC256BadOCSPUrlFromCertTest() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "EC", 256);
        assertThrows(NullPointerException.class, () -> OCSPClient.response(selfSignedCertificate.cert(), selfSignedCertificate.cert()), "Unable to find OCSP URL");
    }

    @Test
    void fetchEC384BadOCSPUrlFromCertTest() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "EC", 384);
        assertThrows(NullPointerException.class, () -> OCSPClient.response(selfSignedCertificate.cert(), selfSignedCertificate.cert()), "Unable to find OCSP URL");
    }

    @Test
    void fetchEC521BadOCSPUrlFromCertTest() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "EC", 521);
        assertThrows(NullPointerException.class, () -> OCSPClient.response(selfSignedCertificate.cert(), selfSignedCertificate.cert()), "Unable to find OCSP URL");
    }

    @Test
    void fetchRSA2048BadOCSPUrlFromCertTest() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "RSA", 2048);
        assertThrows(NullPointerException.class, () -> OCSPClient.response(selfSignedCertificate.cert(), selfSignedCertificate.cert()), "Unable to find OCSP URL");
    }

    @Test
    void fetchRSA3072BadOCSPUrlFromCertTest() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "RSA", 3072);
        assertThrows(NullPointerException.class, () -> OCSPClient.response(selfSignedCertificate.cert(), selfSignedCertificate.cert()), "Unable to find OCSP URL");
    }

    @Test
    void fetchRSA4096BadOCSPUrlFromCertTest() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("www.shieldblaze.com", "RSA", 4096);
        assertThrows(NullPointerException.class, () -> OCSPClient.response(selfSignedCertificate.cert(), selfSignedCertificate.cert()), "Unable to find OCSP URL");
    }
}
