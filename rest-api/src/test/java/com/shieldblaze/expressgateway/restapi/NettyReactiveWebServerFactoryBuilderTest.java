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
package com.shieldblaze.expressgateway.restapi;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.testing.ExpressGatewayConfigured;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import java.io.ByteArrayInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.SecureRandom;

import static com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoStore.fetchPrivateKeyCertificateEntry;
import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NettyReactiveWebServerFactoryBuilderTest {

    @Test
    void generateAndLoadSelfSignedCertificateTest() throws Exception {
        ExpressGateway expressGateway = ExpressGatewayConfigured.forTest();
        ExpressGateway.setInstance(expressGateway);

        // Start the Rest-Api Server
        RestApi.start();

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        SSLSocketFactory factory = sslContext.getSocketFactory();
        SSLSocket socket = (SSLSocket) factory.createSocket("127.0.0.1", 9110);
        assertTrue(socket.isConnected());
        socket.startHandshake();

        SSLSession session = socket.getSession();

        byte[] restApiPkcs12Data = Files.readAllBytes(Path.of(ExpressGateway.getInstance().restApi().PKCS12File()).toAbsolutePath());
        try (ByteArrayInputStream inputStream = new ByteArrayInputStream(restApiPkcs12Data)) {
            CryptoEntry cryptoEntry = fetchPrivateKeyCertificateEntry(inputStream, ExpressGateway.getInstance().restApi().passwordAsChars(), "rest-api");
            assertArrayEquals(cryptoEntry.certificates()[0].getEncoded(), session.getPeerCertificates()[0].getEncoded());
            assertEquals("TLSv1.3", session.getProtocol());
            assertEquals("TLS_AES_256_GCM_SHA384", session.getCipherSuite());
        } finally {
            socket.close();
        }

        // Stop the Rest-Api Server
        RestApi.stop();
    }
}
