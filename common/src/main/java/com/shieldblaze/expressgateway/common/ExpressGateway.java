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
package com.shieldblaze.expressgateway.common;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import io.netty.handler.ssl.ClientAuth;

import java.util.UUID;

public class ExpressGateway {

    @JsonIgnore
    private static volatile ExpressGateway INSTANCE;

    /**
     * Unique Runtime ID
     */
    private final String ID = UUID.randomUUID().toString();

    @JsonProperty("RunningMode")
    private RunningMode runningMode;

    @JsonProperty
    private String ClusterID;

    @JsonProperty("Environment")
    private Environment environment;

    @JsonProperty("Rest-API")
    private RestApi restApi;

    @JsonProperty("ZooKeeper")
    private ZooKeeper zooKeeper;

    @JsonProperty("ServiceDiscovery")
    private ServiceDiscovery serviceDiscovery;

    @JsonProperty("LoadBalancerTLS")
    private LoadBalancerTLS loadBalancerTLS;

    @JsonProperty("ControlPlane")
    private ControlPlane controlPlane;

    public ExpressGateway() {
        // To be used by Jackson Deserializer
    }

    /**
     * This should be used in testing only.
     */
    public ExpressGateway(RunningMode runningMode, String clusterID, Environment environment, RestApi restApi,
                          ZooKeeper zooKeeper, ServiceDiscovery serviceDiscovery, LoadBalancerTLS loadBalancerTLS) {
        this(runningMode, clusterID, environment, restApi, zooKeeper, serviceDiscovery, loadBalancerTLS, null);
    }

    /**
     * This should be used in testing only.
     */
    public ExpressGateway(RunningMode runningMode, String clusterID, Environment environment, RestApi restApi,
                          ZooKeeper zooKeeper, ServiceDiscovery serviceDiscovery, LoadBalancerTLS loadBalancerTLS,
                          ControlPlane controlPlane) {
        this.runningMode = runningMode;
        ClusterID = clusterID;
        this.environment = environment;
        this.restApi = restApi;
        this.zooKeeper = zooKeeper;
        this.serviceDiscovery = serviceDiscovery;
        this.loadBalancerTLS = loadBalancerTLS;
        this.controlPlane = controlPlane;
    }

    public String ID() {
        return ID;
    }

    public RunningMode runningMode() {
        return runningMode;
    }

    public String clusterID() {
        return ClusterID;
    }

    public Environment environment() {
        return environment;
    }

    public RestApi restApi() {
        return restApi;
    }

    public ZooKeeper zooKeeper() {
        return zooKeeper;
    }

    public ServiceDiscovery serviceDiscovery() {
        return serviceDiscovery;
    }

    public LoadBalancerTLS loadBalancerTLS() {
        return loadBalancerTLS;
    }

    public ControlPlane controlPlane() {
        return controlPlane;
    }

    @Override
    public String toString() {
        return "ExpressGateway{" +
                "runningMode=" + runningMode +
                ", Environment='" + environment + '\'' +
                ", ClusterID='" + ClusterID + '\'' +
                ", restApi=" + restApi +
                ", zooKeeper=" + zooKeeper +
                ", serviceDiscovery=" + serviceDiscovery +
                ", loadBalancerTLS=" + loadBalancerTLS +
                ", controlPlane=" + controlPlane +
                '}';
    }

    public static class RestApi {

        @JsonProperty
        private String IPAddress;

        @JsonProperty
        private Integer Port;

        @JsonProperty
        private Boolean EnableTLS;

        @JsonProperty
        private ClientAuth MTLS;

        @JsonProperty
        private String PKCS12File;

        @JsonProperty
        private String Password;

        @JsonIgnore
        private char[] PasswordAsChars;

        public RestApi() {
            // To be used by Jackson Deserializer
        }

        /**
         * This should be used in testing only.
         */
        public RestApi(String IPAddress, Integer port, Boolean enableTLS, ClientAuth MTLS, String PKCS12File, String password) {
            this.IPAddress = IPAddress;
            Port = port;
            EnableTLS = enableTLS;
            this.MTLS = MTLS;
            this.PKCS12File = PKCS12File;
            Password = password;
        }

        public String IPAddress() {
            return IPAddress;
        }

        public Integer port() {
            return Port;
        }

        public Boolean enableTLS() {
            return EnableTLS;
        }

        public ClientAuth mTLS() {
            return MTLS;
        }

        public String PKCS12File() {
            return PKCS12File;
        }

        public char[] passwordAsChars() {
            return PasswordAsChars;
        }

        @Override
        public String toString() {
            return "RestApi{" +
                    "IPAddress='" + IPAddress + '\'' +
                    ", Port=" + Port +
                    ", EnableTLS=" + EnableTLS +
                    ", MTLS=" + MTLS +
                    ", PKCS12File='" + PKCS12File + '\'' +
                    ", Password='*****'}";
        }

        public void clean() {
            if (Password != null) {
                PasswordAsChars = Password.toCharArray();
                Password = null;
            }
        }
    }

    public static class ZooKeeper {

        @JsonProperty
        private String ConnectionString;

        @JsonProperty
        private Integer RetryTimes;

        @JsonProperty
        private Integer SleepMsBetweenRetries;

        @JsonProperty
        private Boolean EnableTLS;

        @JsonProperty
        private Boolean HostnameVerification;

        @JsonProperty
        private String KeyStoreFile;

        @JsonProperty
        private String KeyStorePassword;

        @JsonIgnore
        private char[] KeyStorePasswordAsChars;

        @JsonProperty
        private String TrustStoreFile;

        @JsonProperty
        private String TrustStorePassword;

        @JsonIgnore
        private char[] TrustStorePasswordAsChars;

        public ZooKeeper() {
            // To be used by Jackson Deserializer
        }

        /**
         * This should be used in testing only.
         */
        public ZooKeeper(String connectionString, Integer retryTimes, Integer sleepMsBetweenRetries, Boolean enableTLS, Boolean hostnameVerification,
                         String keyStoreFile, String keyStorePassword, String trustStoreFile, String trustStorePassword) {
            ConnectionString = connectionString;
            RetryTimes = retryTimes;
            SleepMsBetweenRetries = sleepMsBetweenRetries;
            EnableTLS = enableTLS;
            HostnameVerification = hostnameVerification;
            KeyStoreFile = keyStoreFile;
            KeyStorePassword = keyStorePassword;
            TrustStoreFile = trustStoreFile;
            TrustStorePassword = trustStorePassword;
        }

        public String connectionString() {
            return ConnectionString;
        }

        public Integer retryTimes() {
            return RetryTimes;
        }

        public Integer sleepMsBetweenRetries() {
            return SleepMsBetweenRetries;
        }

        public Boolean enableTLS() {
            return EnableTLS;
        }

        public Boolean hostnameVerification() {
            return HostnameVerification;
        }

        public String keyStoreFile() {
            return KeyStoreFile;
        }

        public char[] keyStorePasswordAsChars() {
            return KeyStorePasswordAsChars;
        }

        public String trustStoreFile() {
            return TrustStoreFile;
        }

        public char[] trustStorePasswordAsChars() {
            return TrustStorePasswordAsChars;
        }

        @Override
        public String toString() {
            return "ZooKeeper{" +
                    "ConnectionString='" + ConnectionString + '\'' +
                    ", RetryTimes=" + RetryTimes +
                    ", SleepMsBetweenRetries=" + SleepMsBetweenRetries +
                    ", EnableTLS=" + EnableTLS +
                    ", HostnameVerification=" + HostnameVerification +
                    ", KeyStoreFile='" + KeyStoreFile + '\'' +
                    ", KeyStorePassword='*****'" +
                    ", TrustStoreFile='" + TrustStoreFile + '\'' +
                    ", TrustStorePassword='*****'}";
        }

        public void clean() {
            if (KeyStorePassword != null) {
                KeyStorePasswordAsChars = KeyStorePassword.toCharArray();
                KeyStorePassword = null;
            }

            if (TrustStorePassword != null) {
                TrustStorePasswordAsChars = TrustStorePassword.toCharArray();
                TrustStorePassword = null;
            }
        }
    }

    public static class ServiceDiscovery {

        @JsonProperty
        private String URI;

        @JsonProperty
        private Boolean TrustAllCerts;

        @JsonProperty
        private Boolean HostnameVerification;

        @JsonProperty
        private String KeyStoreFile;

        @JsonProperty
        private String KeyStorePassword;

        @JsonIgnore
        private char[] KeyStorePasswordAsChars;

        @JsonProperty
        private String TrustStoreFile;

        @JsonProperty
        private String TrustStorePassword;

        @JsonIgnore
        private char[] TrustStorePasswordAsChars;

        public ServiceDiscovery() {
            // To be used by Jackson Deserializer
        }

        /**
         * This should be used in testing only.
         */
        public ServiceDiscovery(String URI, Boolean TrustAllCerts, Boolean hostnameVerification, String keyStoreFile, String keyStorePassword,
                                String trustStoreFile, String trustStorePassword) {
            this.URI = URI;
            this.TrustAllCerts = TrustAllCerts;
            HostnameVerification = hostnameVerification;
            KeyStoreFile = keyStoreFile;
            KeyStorePassword = keyStorePassword;
            TrustStoreFile = trustStoreFile;
            TrustStorePassword = trustStorePassword;
        }

        public String URI() {
            return URI;
        }

        public Boolean trustAllCerts() {
            return TrustAllCerts;
        }

        public Boolean hostnameVerification() {
            return HostnameVerification;
        }

        public String keyStoreFile() {
            return KeyStoreFile;
        }

        public char[] keyStorePasswordAsChars() {
            return KeyStorePasswordAsChars;
        }

        public String trustStoreFile() {
            return TrustStoreFile;
        }

        public char[] trustStorePasswordAsChars() {
            return TrustStorePasswordAsChars;
        }

        @Override
        public String toString() {
            return "ServiceDiscovery{" +
                    "URI='" + URI + '\'' +
                    ", TrustAllCerts=" + TrustAllCerts +
                    ", HostnameVerification=" + HostnameVerification +
                    ", KeyStoreFile='" + KeyStoreFile + '\'' +
                    ", KeyStorePassword='*****'" +
                    ", TrustStoreFile='" + TrustStoreFile + '\'' +
                    ", TrustStorePassword='*****'}";
        }

        public void clean() {
            if (KeyStorePassword != null) {
                KeyStorePasswordAsChars = KeyStorePassword.toCharArray();
                KeyStorePassword = null;
            }

            if (TrustStorePassword != null) {
                TrustStorePasswordAsChars = TrustStorePassword.toCharArray();
                TrustStorePassword = null;
            }
        }
    }

    public static class LoadBalancerTLS {

        @JsonProperty
        private Boolean EnableTLS;

        @JsonProperty
        private String PKCS12File;

        @JsonProperty
        private String Password;

        @JsonIgnore
        private char[] PasswordAsChars;

        public LoadBalancerTLS() {
            // To be used by Jackson Deserializer
        }

        /**
         * This should be used in testing only.
         */
        public LoadBalancerTLS(Boolean enableTLS, String PKCS12File, String password) {
            EnableTLS = enableTLS;
            this.PKCS12File = PKCS12File;
            Password = password;
        }

        public Boolean enableTLS() {
            return EnableTLS;
        }

        public String PKCS12File() {
            return PKCS12File;
        }

        public char[] passwordAsChars() {
            return PasswordAsChars;
        }

        @Override
        public String toString() {
            return "LoadBalancerTLS{" +
                    "EnableTLS='" + EnableTLS + '\'' +
                    "PKCS12File='" + PKCS12File + '\'' +
                    ", Password='*****'}";
        }

        public void clean() {
            if (Password != null) {
                PasswordAsChars = Password.toCharArray();
                Password = null;
            }
        }
    }

    public static class ControlPlane {

        @JsonProperty
        private Boolean Enabled;

        @JsonProperty
        private Integer GrpcPort;

        @JsonProperty
        private String GrpcBindAddress;

        @JsonProperty
        private String KvStoreType; // "ZOOKEEPER", "CONSUL", "ETCD"

        @JsonProperty
        private Long HeartbeatIntervalMs;

        @JsonProperty
        private Integer HeartbeatMissThreshold;

        @JsonProperty
        private Long WriteBatchWindowMs;

        @JsonProperty
        private Integer MaxNodes;

        // -- Control Plane Agent settings (for data plane nodes) --

        @JsonProperty
        private String ControlPlaneAddress; // Address to connect to when acting as data plane node

        @JsonProperty
        private Integer ControlPlanePort; // Port to connect to

        public ControlPlane() {
            // To be used by Jackson Deserializer
        }

        public Boolean enabled() {
            return Enabled;
        }

        public Integer grpcPort() {
            return GrpcPort;
        }

        public String grpcBindAddress() {
            return GrpcBindAddress;
        }

        public String kvStoreType() {
            return KvStoreType;
        }

        public Long heartbeatIntervalMs() {
            return HeartbeatIntervalMs;
        }

        public Integer heartbeatMissThreshold() {
            return HeartbeatMissThreshold;
        }

        public Long writeBatchWindowMs() {
            return WriteBatchWindowMs;
        }

        public Integer maxNodes() {
            return MaxNodes;
        }

        public String controlPlaneAddress() {
            return ControlPlaneAddress;
        }

        public Integer controlPlanePort() {
            return ControlPlanePort;
        }

        private static final java.util.Set<String> VALID_KV_STORE_TYPES =
                java.util.Set.of("ZOOKEEPER", "CONSUL", "ETCD");

        /**
         * Validates the ControlPlane configuration.
         *
         * @throws IllegalStateException if required fields are invalid
         */
        public void validate() {
            if (Boolean.TRUE.equals(Enabled)) {
                if (GrpcPort != null && (GrpcPort < 1 || GrpcPort > 65535)) {
                    throw new IllegalStateException("ControlPlane GrpcPort must be in range [1, 65535], got: " + GrpcPort);
                }
                if (KvStoreType != null && !VALID_KV_STORE_TYPES.contains(KvStoreType)) {
                    throw new IllegalStateException("ControlPlane KvStoreType must be one of: ZOOKEEPER, CONSUL, ETCD; got: " + KvStoreType);
                }
            }
            if (ControlPlaneAddress != null) {
                if (ControlPlanePort == null || ControlPlanePort < 1 || ControlPlanePort > 65535) {
                    throw new IllegalStateException("ControlPlane ControlPlanePort must be in range [1, 65535] when ControlPlaneAddress is set");
                }
            }
        }

        @Override
        public String toString() {
            return "ControlPlane{" +
                    "Enabled=" + Enabled +
                    ", GrpcPort=" + GrpcPort +
                    ", GrpcBindAddress='" + GrpcBindAddress + '\'' +
                    ", KvStoreType='" + KvStoreType + '\'' +
                    ", HeartbeatIntervalMs=" + HeartbeatIntervalMs +
                    ", HeartbeatMissThreshold=" + HeartbeatMissThreshold +
                    ", WriteBatchWindowMs=" + WriteBatchWindowMs +
                    ", MaxNodes=" + MaxNodes +
                    ", ControlPlaneAddress='" + ControlPlaneAddress + '\'' +
                    ", ControlPlanePort=" + ControlPlanePort +
                    '}';
        }
    }

    public enum RunningMode {

        /**
         * Standalone mode runs on 1 single node without the support of Apache ZooKeeper.
         */
        STANDALONE,

        /**
         * Replica mode uses replication to run on multiple nodes in
         * a cluster. It uses Apache ZooKeeper for coordination.
         */
        REPLICA
    }

    /**
     * Validates the configuration after deserialization.
     * Fails fast with clear error messages if required fields are missing.
     *
     * @throws IllegalStateException if validation fails
     */
    public void validate() {
        if (runningMode == null) {
            throw new IllegalStateException("RunningMode must not be null");
        }
        if (environment == null) {
            throw new IllegalStateException("Environment must not be null");
        }
        if (runningMode == RunningMode.REPLICA) {
            if (zooKeeper == null) {
                throw new IllegalStateException("ZooKeeper configuration required in REPLICA mode");
            }
            if (zooKeeper.connectionString() == null || zooKeeper.connectionString().isEmpty()) {
                throw new IllegalStateException("ZooKeeper ConnectionString must not be empty in REPLICA mode");
            }
            if (serviceDiscovery == null) {
                throw new IllegalStateException("ServiceDiscovery configuration required in REPLICA mode");
            }
            if (controlPlane != null) {
                controlPlane.validate();
            }
        }
    }

    public static void setInstance(ExpressGateway expressGateway) {
        INSTANCE = expressGateway;
        INSTANCE.cleanSensitiveData();
    }

    public static ExpressGateway getInstance() {
        return INSTANCE;
    }

    public void cleanSensitiveData() {
        if (restApi() != null) restApi().clean();
        if (zooKeeper() != null) zooKeeper().clean();
        if (serviceDiscovery() != null) serviceDiscovery().clean();
        if (loadBalancerTLS() != null) loadBalancerTLS().clean();
    }
}
