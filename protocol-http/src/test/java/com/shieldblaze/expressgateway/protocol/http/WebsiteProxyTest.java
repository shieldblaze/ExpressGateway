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
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import okhttp3.ConnectionPool;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.ResponseBody;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import javax.net.ssl.X509TrustManager;
import java.net.InetSocketAddress;
import java.security.SecureRandom;
import java.util.List;

import static org.assertj.core.api.Assertions.assertThat;

public class WebsiteProxyTest {

    private static final Logger logger = LogManager.getLogger(WebsiteProxyTest.class);

    private static final List<String> WEBSITES = List.of(
            "www.shieldblaze.com"
    );

    private static final OkHttpClient OK_HTTP_CLIENT;

    static {
        try {
            SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
            sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

            OK_HTTP_CLIENT = new OkHttpClient().newBuilder()
                    .sslSocketFactory(sslContext.getSocketFactory(), (X509TrustManager) InsecureTrustManagerFactory.INSTANCE.getTrustManagers()[0])
                    .followRedirects(false)
                    .followSslRedirects(false)
                    .build();
        } catch (Exception ex) {
            throw new RuntimeException(ex);
        }
    }

    private static int LoadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;

    @BeforeAll
    static void setup() throws Exception {
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), WEBSITES);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

        TlsClientConfiguration tlsClientConfiguration = TlsClientConfiguration.DEFAULT;
        TlsServerConfiguration tlsServerConfiguration = TlsServerConfiguration.DEFAULT;

        tlsServerConfiguration.enable();
        tlsServerConfiguration.addMapping("localhost", certificateKeyPair);
        tlsServerConfiguration.defaultMapping(certificateKeyPair);

        tlsClientConfiguration.enable();
        tlsClientConfiguration.setAcceptAllCerts(true);
        tlsClientConfiguration.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        LoadBalancerPort = AvailablePortUtil.getTcpPort();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(tlsClientConfiguration, tlsServerConfiguration))
                .withBindAddress(new InetSocketAddress("127.0.0.1", LoadBalancerPort))
                .withL4FrontListener(new TCPListener())
                .withName("HttpLoadBalancer")
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().get();

        for (String domain : WEBSITES) {
            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress(domain, 443))
                    .build();

            httpLoadBalancer.mapCluster(domain, cluster);
        }
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
    }

    @Test
    void loadWebsitesExpect200To399AndValidateBody() throws Exception {
        for (String domain : WEBSITES) {
            System.out.println("Connecting to: " + domain);

            Request request = new Request.Builder()
                    .get()
                    .url("https://127.0.0.1:" + LoadBalancerPort + '/')
                    .header("Host", domain)
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/106.0.0.0 Safari/537.36")
                    .build();

            try {
                for (int i = 0; i < 5; i++) {
                    try (Response response = OK_HTTP_CLIENT.newCall(request).execute()) {
                        assertThat(response.code()).isBetween(200, 399);

                        ResponseBody responseBody = response.body();
                        assertThat(responseBody).isNotNull();
                        assertThat(responseBody.bytes()).isNotNull();

                        logger.info("Domain: {}; Successful", domain);
                    } catch (Exception ex) {
                        logger.error("Failed Domain Proxy: {}, Reason: {}", domain, ex.getMessage());
                        throw ex;
                    }
                }
            } finally {
                ConnectionPool connectionPool = OK_HTTP_CLIENT.connectionPool();
                connectionPool.evictAll();
                assertThat(connectionPool.connectionCount()).isEqualTo(0);
                logger.info("Closed all connections in ConnectionPool for Domain: {}", domain);
            }

            Thread.sleep(250);
        }
    }
}
