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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.*;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLContext;
import java.net.InetSocketAddress;
import java.net.http.HttpClient;
import java.security.SecureRandom;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertTrue;

public final class Common {

    private static final Logger logger = LogManager.getLogger(Common.class);

    public static HttpClient httpClient;
    private static HTTPServer httpServer;
    private static HTTPLoadBalancer httpLoadBalancer;

    public static void initialize() throws Exception {
        initialize(true, true, true);
    }

    public static void initialize(boolean tlsBackend, boolean tlsServer, boolean tlsClient) throws Exception {
        logger.debug("Initializing New HTTP web server");
        logger.debug("TLSBackend: {}, TLSServer: {}, TLSClient: {}", tlsBackend, tlsServer, tlsClient);

        SelfSignedCertificate ssc = new SelfSignedCertificate("localhost", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(Collections.singletonList(ssc.cert()), ssc.key());

        TLSConfiguration forServer = TLSServerConfiguration.DEFAULT;
        if (tlsServer) {
            forServer = TLSConfigurationBuilder.copy(TLSServerConfiguration.DEFAULT);
        }
        forServer.addMapping("localhost", certificateKeyPair);

        TLSConfiguration forClient = TLSClientConfiguration.DEFAULT;
        if (tlsClient) {
            forClient = TLSConfigurationBuilder.forClient()
                    .withAcceptAllCertificate(true)
                    .build();
        }
        forClient.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        httpClient = HttpClient.newBuilder()
                .sslContext(sslContext)
                .build();

        httpServer = new HTTPServer(tlsBackend, new Handler());
        httpServer.start();
        Thread.sleep(2500);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(null, forClient, forServer))
                .withBindAddress(new InetSocketAddress("localhost", 9110))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:9110", cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("localhost", httpServer.port()))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccess());
    }

    public static void shutdown() {
        logger.debug("Shutting down HTTP web server");

        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());

        try {
            Thread.sleep(2500);
        } catch (InterruptedException e) {
            e.printStackTrace();
        }
    }
}
