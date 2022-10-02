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

import com.shieldblaze.expressgateway.common.datastore.CryptoEntry;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class WebServerCustomizerTest {

    @Test
    void generateAndLoadSelfSignedCertificateTest() throws Exception {
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("shieldblaze.com"));
        CryptoEntry cryptoEntry = new CryptoEntry(ssc.keyPair().getPrivate(), new X509Certificate[]{ssc.x509Certificate()});

        // Start the Rest-Api Server
        RestApi.start(cryptoEntry);

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        SSLSocketFactory factory = sslContext.getSocketFactory();
        SSLSocket socket = (SSLSocket) factory.createSocket("127.0.0.1", 9110);
        assertTrue(socket.isConnected());
        socket.startHandshake();

        SSLSession session = socket.getSession();

        assertArrayEquals(cryptoEntry.certificates()[0].getEncoded(), session.getPeerCertificates()[0].getEncoded());
        assertEquals("TLSv1.3", session.getProtocol());
        assertEquals("TLS_AES_256_GCM_SHA384", session.getCipherSuite());
        socket.close();

        // Stop the Rest-Api Server
        RestApi.stop();
    }
}
