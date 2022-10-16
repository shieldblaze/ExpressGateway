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
import com.shieldblaze.expressgateway.common.curator.Environment;

public class ExpressGateway {

    @JsonIgnore
    private static ExpressGateway INSTANCE;

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

    @JsonProperty("LoadBalancerTLS")
    private LoadBalancerTLS loadBalancerTLS;

    public ExpressGateway() {
    }

    /**
     * This should be used in testing only.
     */
    public ExpressGateway(RunningMode runningMode, String clusterID, Environment environment, RestApi restApi,
                          ZooKeeper zooKeeper, LoadBalancerTLS loadBalancerTLS) {
        this.runningMode = runningMode;
        ClusterID = clusterID;
        this.environment = environment;
        this.restApi = restApi;
        this.zooKeeper = zooKeeper;
        this.loadBalancerTLS = loadBalancerTLS;
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

    public LoadBalancerTLS loadBalancerTLS() {
        return loadBalancerTLS;
    }

    @Override
    public String toString() {
        return "ExpressGateway{" +
                "runningMode=" + runningMode +
                ", Environment='" + environment + '\'' +
                ", ClusterID='" + ClusterID + '\'' +
                ", restApi=" + restApi +
                ", zooKeeper=" + zooKeeper +
                ", loadBalancerTLS=" + loadBalancerTLS +
                '}';
    }

    public static class RestApi implements CleanSensitiveData {

        @JsonProperty
        private String IPAddress;

        @JsonProperty
        private Integer Port;

        @JsonProperty
        private Boolean EnableTLS;

        @JsonProperty
        private String PKCS12File;

        @JsonProperty
        private String Password;

        @JsonIgnore
        private char[] PasswordAsChars;

        public RestApi() {
        }

        /**
         * This should be used in testing only.
         */
        public RestApi(String IPAddress, Integer port, Boolean enableTLS, String PKCS12File, String password) {
            this.IPAddress = IPAddress;
            Port = port;
            EnableTLS = enableTLS;
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
                    ", PKCS12File='" + PKCS12File + '\'' +
                    ", Password=*****" +
                    '}';
        }

        @Override
        public void clean() {
            PasswordAsChars = Password.toCharArray();
            Password = null;
        }
    }

    public static class ZooKeeper implements CleanSensitiveData {

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
                    ", KeyStorePassword=*****" +
                    ", TrustStoreFile='" + TrustStoreFile + '\'' +
                    ", TrustStorePassword=*****" +
                    '}';
        }

        @Override
        public void clean() {
            KeyStorePasswordAsChars = KeyStorePassword.toCharArray();
            KeyStorePassword = null;

            TrustStorePasswordAsChars = TrustStorePassword.toCharArray();
            TrustStorePassword = null;
        }
    }

    public static class LoadBalancerTLS implements CleanSensitiveData {

        @JsonProperty
        private Boolean EnableTLS;

        @JsonProperty
        private String PKCS12File;

        @JsonProperty
        private String Password;

        @JsonIgnore
        private char[] PasswordAsChars;

        public LoadBalancerTLS() {
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
                    ", Password=*****" +
                    '}';
        }

        @Override
        public void clean() {
            PasswordAsChars = Password.toCharArray();
            Password = null;
        }
    }

    public enum RunningMode {

        /**
         * Standalone mode runs on 1 single node without the
         * support of MongoDB database and Apache ZooKeeper.
         */
        STANDALONE,

        /**
         * Replica mode uses replication to run on multiple nodes in
         * a cluster. It uses Apache ZooKeeper for coordination.
         */
        REPLICA
    }

    public static void setInstance(ExpressGateway expressGateway) {
        INSTANCE = expressGateway;
        INSTANCE.cleanSensitiveData();
    }

    public static ExpressGateway getInstance() {
        return INSTANCE;
    }

    private interface CleanSensitiveData {
        void clean();
    }

    public void cleanSensitiveData() {
        restApi().clean();
        zooKeeper().clean();
        loadBalancerTLS().clean();

        // Run GC to wipe all sensitive information
        System.gc();
    }
}
