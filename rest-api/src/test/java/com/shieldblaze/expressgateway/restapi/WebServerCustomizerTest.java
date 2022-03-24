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

import com.shieldblaze.expressgateway.common.datastore.DataStore;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;

import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import java.net.Socket;
import java.security.SecureRandom;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class WebServerCustomizerTest {

    @Test
    void generateAndLoadSelfSignedCertificateTest() throws Exception {
        final String password = "MeowMeowCatCat";
        final String alias = "meowAlias";

        SelfSignedCertificate selfSignedCertificate = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("localhost"));
        DataStore.INSTANCE.store(password.toCharArray(), alias, selfSignedCertificate.keyPair().getPrivate(), selfSignedCertificate.x509Certificate());
        System.setProperty("datastore.alias", alias);
        System.setProperty("datastore.password", password);

        // Start the Rest-Api Server
        RestAPI.start();

        // Wait 2.5 seconds for everything to be initialized
        Thread.sleep(2500);

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        SSLSocketFactory factory = sslContext.getSocketFactory();
        SSLSocket socket = (SSLSocket) factory.createSocket("127.0.0.1", 9110);
        assertTrue(socket.isConnected());
        socket.startHandshake();

        SSLSession session = socket.getSession();

        assertArrayEquals(selfSignedCertificate.x509Certificate().getEncoded(), session.getPeerCertificates()[0].getEncoded());
        socket.close();

        assertTrue(DataStore.INSTANCE.destroy());
    }
}
