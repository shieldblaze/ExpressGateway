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
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;

import javax.net.ssl.SSLContext;
import java.io.Closeable;
import java.net.InetSocketAddress;
import java.net.http.HttpClient;
import java.security.SecureRandom;
import java.util.List;
import java.util.concurrent.ExecutionException;

import static org.junit.jupiter.api.Assertions.assertTrue;

public final class TestableHttpLoadBalancer implements Closeable {

    private boolean tlsServerEnabled;
    private boolean tlsClientEnabled;
    private boolean tlsBackendEnabled;
    private HTTPLoadBalancer httpLoadBalancer;
    private HttpServer httpServer;
    private HttpClient httpClient;

    public void start() throws Exception {
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("localhost"));
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

        TlsClientConfiguration tlsClientConfiguration = TlsClientConfiguration.DEFAULT;
        TlsServerConfiguration tlsServerConfiguration = TlsServerConfiguration.DEFAULT;

        if (tlsServerEnabled) {
            tlsServerConfiguration.enable();
        }
        tlsServerConfiguration.addMapping("localhost", certificateKeyPair);

        if (tlsClientEnabled) {
            tlsClientConfiguration.enable();
            tlsClientConfiguration.setAcceptAllCerts(true);
        }
        tlsClientConfiguration.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        httpClient = HttpClient.newBuilder()
                .sslContext(sslContext)
                .build();

        httpServer = new HttpServer(tlsBackendEnabled);
        httpServer.start();
        httpServer.START_FUTURE.get();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(tlsClientConfiguration, tlsServerConfiguration))
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

    public HttpClient httpClient() {
        return httpClient;
    }

    @Override
    public void close() {
        httpServer.shutdown();
        try {
            httpServer.SHUTDOWN_FUTURE.get();
        } catch (InterruptedException | ExecutionException e) {
            throw new RuntimeException(e);
        }

        TlsServerConfiguration.DEFAULT.disable();
        TlsServerConfiguration.DEFAULT.clearMappings();

        TlsClientConfiguration.DEFAULT.disable();
        TlsClientConfiguration.DEFAULT.clearMappings();

        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());
    }

    public static final class Builder {
        private boolean tlsServerEnabled;
        private boolean tlsClientEnabled;
        private boolean tlsBackendEnabled;

        private Builder() {
            // Prevent outside initialization
        }

        public static Builder newBuilder() {
            return new Builder();
        }

        public Builder withTlsServerEnabled(boolean tlsServerEnabled) {
            this.tlsServerEnabled = tlsServerEnabled;
            return this;
        }

        public Builder withTlsClientEnabled(boolean tlsClientEnabled) {
            this.tlsClientEnabled = tlsClientEnabled;
            return this;
        }

        public Builder withTlsBackendEnabled(boolean tlsBackendEnabled) {
            this.tlsBackendEnabled = tlsBackendEnabled;
            return this;
        }

        public TestableHttpLoadBalancer build() throws Exception {
            TestableHttpLoadBalancer testableHttpLoadBalancer = new TestableHttpLoadBalancer();
            testableHttpLoadBalancer.tlsClientEnabled = this.tlsClientEnabled;
            testableHttpLoadBalancer.tlsServerEnabled = this.tlsServerEnabled;
            testableHttpLoadBalancer.tlsBackendEnabled = this.tlsBackendEnabled;
            return testableHttpLoadBalancer;
        }
    }
}
