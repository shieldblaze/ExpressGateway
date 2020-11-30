/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.microbench.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.Test;
import org.openjdk.jmh.annotations.Benchmark;
import org.openjdk.jmh.annotations.BenchmarkMode;
import org.openjdk.jmh.annotations.Mode;
import org.openjdk.jmh.annotations.Scope;
import org.openjdk.jmh.annotations.Setup;
import org.openjdk.jmh.annotations.State;
import org.openjdk.jmh.annotations.Warmup;
import org.openjdk.jmh.infra.Blackhole;
import org.openjdk.jmh.runner.Runner;
import org.openjdk.jmh.runner.RunnerException;
import org.openjdk.jmh.runner.options.Options;
import org.openjdk.jmh.runner.options.OptionsBuilder;

import java.net.InetSocketAddress;

@State(Scope.Benchmark)
@BenchmarkMode(Mode.Throughput)
@Warmup(iterations = 5)
public class RoundRobinBenchTest {

    private static final Logger logger = LogManager.getLogger(RoundRobinBenchTest.class);

    private Cluster cluster10;
    private Cluster cluster50;
    private Cluster cluster100;

    @Test
    void runBenchmark() throws RunnerException {
        if (System.getProperty("skipBench") != null && Boolean.parseBoolean(System.getProperty("skipBench"))) {
            logger.info("\"skipBench\" is set to false, skipping benchmarking test.");
        } else {
            Options opt = new OptionsBuilder()
                    .include(RoundRobinBenchTest.class.getSimpleName())
                    .forks(5)
                    .addProfiler("gc")
                    .build();

            new Runner(opt).run();
        }
    }

    @Setup
    public void setup() {
        cluster10 = new ClusterPool(new EventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE), "ClusterBench10");
        cluster50 = new ClusterPool(new EventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE), "ClusterBench50");
        cluster100 = new ClusterPool(new EventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE), "ClusterBench100");

        for (int i = 1; i <= 10; i++) {
            new Node(cluster10, new InetSocketAddress("10.10.1." + i, i));
        }

        for (int i = 1; i <= 50; i++) {
            new Node(cluster50, new InetSocketAddress("10.11.1." + i, i));
        }

        for (int i = 1; i <= 100; i++) {
            new Node(cluster100, new InetSocketAddress("10.12.1." + i, i));
        }
    }

    @Benchmark
    public void cluster10Bench(Blackhole blackhole) throws LoadBalanceException {
        blackhole.consume(cluster10.nextNode(new L4Request(new InetSocketAddress("192.168.1.1", 1))));
    }

    @Benchmark
    public void cluster50Bench(Blackhole blackhole) throws LoadBalanceException {
        blackhole.consume(cluster50.nextNode(new L4Request(new InetSocketAddress("192.168.1.1", 1))));
    }

    @Benchmark
    public void cluster100Bench(Blackhole blackhole) throws LoadBalanceException {
        blackhole.consume(cluster100.nextNode(new L4Request(new InetSocketAddress("192.168.1.1", 1))));
    }
}