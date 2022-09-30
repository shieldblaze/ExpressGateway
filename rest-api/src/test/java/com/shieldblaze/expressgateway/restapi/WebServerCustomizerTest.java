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

import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import java.security.SecureRandom;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class WebServerCustomizerTest {

    @Test
    void generateAndLoadSelfSignedCertificateTest() throws Exception {
        Utils.initSelfSignedDataStore();

        // Start the Rest-Api Server
        RestApi.start();

        // Wait 2.5 seconds for everything to be initialized
        Thread.sleep(2500);

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        SSLSocketFactory factory = sslContext.getSocketFactory();
        SSLSocket socket = (SSLSocket) factory.createSocket("127.0.0.1", 9110);
        assertTrue(socket.isConnected());
        socket.startHandshake();

        SSLSession session = socket.getSession();

        assertArrayEquals(Utils.selfSignedCertificate().x509Certificate().getEncoded(), session.getPeerCertificates()[0].getEncoded());
        assertEquals("TLSv1.3", session.getProtocol());
        assertEquals("TLS_AES_256_GCM_SHA384", session.getCipherSuite());
        socket.close();

//        assertTrue(DataStore.INSTANCE.destroy());

        // Stop the Rest-Api Server
        RestApi.stop();
    }
}
