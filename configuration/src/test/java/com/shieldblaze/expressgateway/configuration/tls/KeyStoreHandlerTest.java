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
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.security.cert.CertificateException;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class KeyStoreHandlerTest {

    private static SelfSignedCertificate ssc;

    @BeforeAll
    static void setup() throws CertificateException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));
        ssc = new SelfSignedCertificate();
    }

    @Test
    @Order(1)
    void saveCertAndKey() throws Exception {
        TLSConfiguration tlsConfiguration = TLSConfigurationBuilder.forServer()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withSessionTimeout(100)
                .withSessionCacheSize(200)
                .build();

        tlsConfiguration.defaultMapping(new CertificateKeyPair(Collections.singletonList(ssc.cert()), ssc.key()));
        KeyStoreHandler.saveServer(tlsConfiguration, "MeowProfile", "Meow");
    }

    @Test
    @Order(2)
    void loadCertAndKey() throws Exception {
        TLSConfiguration tlsConfiguration = TLSConfigurationBuilder.forClient()
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .build();

        KeyStoreHandler.loadServer(tlsConfiguration, "MeowProfile", "Meow");
        assertArrayEquals(ssc.cert().getEncoded(), tlsConfiguration.defaultMapping().certificates().get(0).getEncoded());
    }
}
