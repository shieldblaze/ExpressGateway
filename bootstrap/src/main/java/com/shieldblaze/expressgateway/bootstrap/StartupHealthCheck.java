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
package com.shieldblaze.expressgateway.bootstrap;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import lombok.extern.slf4j.Slf4j;

import java.util.ArrayList;
import java.util.List;

/**
 * Verifies all required subsystems are initialized and healthy before the gateway
 * begins accepting traffic. Each check is a named predicate that returns pass/fail.
 *
 * <p>The startup health check runs after all initialization tasks complete and
 * before the gateway advertises itself as ready (e.g., via service discovery).
 * If any critical check fails, the gateway should abort startup.</p>
 */
@Slf4j
public final class StartupHealthCheck {

    /**
     * A single health check with a name and a check function.
     */
    public record Check(String name, CheckFunction fn) {
    }

    /**
     * A function that returns true if the check passes.
     */
    @FunctionalInterface
    public interface CheckFunction {
        boolean check();
    }

    /**
     * Result of running all health checks.
     */
    public record Result(boolean healthy, List<String> passed, List<String> failed) {
    }

    /**
     * Run all startup health checks appropriate for the current running mode.
     *
     * @return the overall result
     */
    public static Result runChecks() {
        List<Check> checks = new ArrayList<>();

        // Always check: ExpressGateway instance is configured
        checks.add(new Check("ExpressGateway instance configured",
                () -> ExpressGateway.getInstance() != null));

        // REPLICA mode checks
        if (ExpressGateway.getInstance() != null &&
                ExpressGateway.getInstance().runningMode() == ExpressGateway.RunningMode.REPLICA) {

            checks.add(new Check("Curator/ZooKeeper initialized",
                    () -> Curator.isInitialized().getNow(false)));
        }

        return executeChecks(checks);
    }

    /**
     * Execute a list of checks and return the aggregated result.
     */
    static Result executeChecks(List<Check> checks) {
        List<String> passed = new ArrayList<>();
        List<String> failed = new ArrayList<>();

        for (Check check : checks) {
            try {
                if (check.fn().check()) {
                    passed.add(check.name());
                    log.info("Health check PASSED: {}", check.name());
                } else {
                    failed.add(check.name());
                    log.error("Health check FAILED: {}", check.name());
                }
            } catch (Exception ex) {
                failed.add(check.name() + " (exception: " + ex.getMessage() + ")");
                log.error("Health check EXCEPTION: {}", check.name(), ex);
            }
        }

        boolean healthy = failed.isEmpty();
        if (healthy) {
            log.info("All {} startup health checks passed", passed.size());
        } else {
            log.error("{} of {} startup health checks failed", failed.size(), checks.size());
        }

        return new Result(healthy, List.copyOf(passed), List.copyOf(failed));
    }
}
