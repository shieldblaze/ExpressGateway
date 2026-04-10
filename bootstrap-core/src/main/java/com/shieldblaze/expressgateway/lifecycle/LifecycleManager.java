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
package com.shieldblaze.expressgateway.lifecycle;

import com.shieldblaze.expressgateway.config.GatewayConfig;
import com.shieldblaze.expressgateway.config.RunningMode;
import lombok.extern.log4j.Log4j2;

import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Lifecycle orchestrator for the gateway. Manages deterministic startup and shutdown
 * through a sequence of well-defined {@link LifecyclePhase phases}.
 *
 * <p>This class is NOT a singleton. It is instantiated with a {@link GatewayConfig}
 * and manages an ordered list of {@link LifecycleParticipant participants}.</p>
 *
 * <h2>Startup sequence</h2>
 * <ol>
 *   <li>{@link LifecyclePhase#INITIALIZE} - Config already loaded (passed in constructor). Log summary.</li>
 *   <li>{@link LifecyclePhase#CONSTRUCT} - Phase marker; components registered externally.</li>
 *   <li>{@link LifecyclePhase#CONNECT} - Start participants registered for CONNECT phase (coordination).</li>
 *   <li>{@link LifecyclePhase#START} - Start all remaining participants sorted by priority (ascending).</li>
 *   <li>Transition to {@link LifecyclePhase#RUNNING}.</li>
 * </ol>
 *
 * <h2>Shutdown sequence</h2>
 * <ol>
 *   <li>{@link LifecyclePhase#DRAIN} - Notify drain-phase participants.</li>
 *   <li>{@link LifecyclePhase#SHUTDOWN} - Stop all participants in reverse priority order.</li>
 * </ol>
 *
 * <p>If any phase fails during startup, already-started components are shut down
 * in reverse order (rollback). Thread-safe state transitions are managed via
 * {@link AtomicReference}.</p>
 */
@Log4j2
public final class LifecycleManager {

    private final GatewayConfig config;
    private final AtomicReference<LifecyclePhase> currentPhase = new AtomicReference<>(null);
    private final CopyOnWriteArrayList<LifecycleParticipant> participants = new CopyOnWriteArrayList<>();
    private final CopyOnWriteArrayList<LifecycleListener> listeners = new CopyOnWriteArrayList<>();

    /**
     * Tracks participants that have been successfully started, in start order.
     * Used for rollback on failure and orderly shutdown.
     */
    private final List<LifecycleParticipant> startedParticipants = new ArrayList<>();

    /**
     * Creates a new lifecycle manager for the given configuration.
     *
     * @param config the validated gateway configuration
     */
    public LifecycleManager(GatewayConfig config) {
        this.config = Objects.requireNonNull(config, "config must not be null");
    }

    /**
     * Registers a lifecycle participant. Must be called before {@link #startup()}.
     *
     * @param participant the participant to register
     * @throws IllegalStateException if the manager has already started
     */
    public void register(LifecycleParticipant participant) {
        Objects.requireNonNull(participant, "participant must not be null");
        LifecyclePhase phase = currentPhase.get();
        if (phase != null && phase.ordinal() >= LifecyclePhase.START.ordinal()) {
            throw new IllegalStateException(
                    "Cannot register participants after startup has begun (current phase: " + phase + ")");
        }
        participants.add(participant);
        log.debug("Registered lifecycle participant: {} (phase={}, priority={})",
                participant.name(), participant.phase(), participant.priority());
    }

    /**
     * Registers a lifecycle listener for phase transition notifications.
     *
     * @param listener the listener to register
     */
    public void addListener(LifecycleListener listener) {
        Objects.requireNonNull(listener, "listener must not be null");
        listeners.add(listener);
    }

    /**
     * Executes the startup sequence through all phases.
     *
     * @throws LifecycleException if any phase fails
     */
    public void startup() throws LifecycleException {
        if (!currentPhase.compareAndSet(null, LifecyclePhase.INITIALIZE)) {
            throw new LifecycleException(LifecyclePhase.INITIALIZE,
                    "Lifecycle manager already started or starting (current phase: " + currentPhase.get() + ")");
        }

        Instant totalStart = Instant.now();

        try {
            // Phase 1: INITIALIZE
            executePhaseTransition(null, LifecyclePhase.INITIALIZE, () -> {
                log.info("Gateway configuration loaded: clusterId={}, mode={}, environment={}",
                        config.getClusterId(), config.getRunningMode(), config.getEnvironment());
                if (config.getRestApi() != null) {
                    log.info("REST API: {}:{} (TLS={})",
                            config.getRestApi().getBindAddress(),
                            config.getRestApi().getPort(),
                            config.getRestApi().isTlsEnabled());
                }
                if (config.getCoordination() != null) {
                    log.info("Coordination: backend={}, endpoints={}",
                            config.getCoordination().getBackend(),
                            config.getCoordination().getEndpoints());
                }
            });

            // Phase 2: CONSTRUCT
            executePhaseTransition(LifecyclePhase.INITIALIZE, LifecyclePhase.CONSTRUCT, () ->
                    log.info("Construct phase: {} participant(s) registered", participants.size()));

            // Phase 3: CONNECT
            executePhaseTransition(LifecyclePhase.CONSTRUCT, LifecyclePhase.CONNECT, () -> {
                if (config.getRunningMode() == RunningMode.CLUSTERED) {
                    startParticipantsForPhase(LifecyclePhase.CONNECT);
                } else {
                    log.info("Standalone mode - skipping coordination connection");
                }
            });

            // Phase 4: START
            executePhaseTransition(LifecyclePhase.CONNECT, LifecyclePhase.START, () ->
                    startParticipantsForPhase(LifecyclePhase.START));

            // Phase 5: RUNNING
            executePhaseTransition(LifecyclePhase.START, LifecyclePhase.RUNNING, () -> {
                Duration totalDuration = Duration.between(totalStart, Instant.now());
                log.info("Gateway startup complete in {} ms ({} participant(s) running)",
                        totalDuration.toMillis(), startedParticipants.size());
            });

        } catch (LifecycleException e) {
            log.error("Startup failed in phase {}: {}", e.getPhase(), e.getMessage(), e);
            rollbackStartedComponents();
            throw e;
        }
    }

    /**
     * Executes the shutdown sequence: DRAIN then SHUTDOWN.
     */
    public void shutdown() {
        LifecyclePhase phase = currentPhase.get();
        if (phase == null || phase == LifecyclePhase.SHUTDOWN) {
            log.debug("Shutdown called but lifecycle not running or already shut down");
            return;
        }

        log.info("Initiating gateway shutdown from phase: {}", phase);

        // Phase: DRAIN
        try {
            LifecyclePhase previousPhase = currentPhase.getAndSet(LifecyclePhase.DRAIN);
            Instant drainStart = Instant.now();
            notifyDrainParticipants();
            Duration drainDuration = Duration.between(drainStart, Instant.now());
            fireEvent(new LifecycleEvent(LifecyclePhase.DRAIN, previousPhase, null, drainDuration));
            log.info("Drain phase complete in {} ms", drainDuration.toMillis());
        } catch (Exception e) {
            log.warn("Error during drain phase (continuing with shutdown)", e);
        }

        // Phase: SHUTDOWN
        LifecyclePhase previousPhase = currentPhase.getAndSet(LifecyclePhase.SHUTDOWN);
        Instant shutdownStart = Instant.now();
        stopAllParticipants();
        Duration shutdownDuration = Duration.between(shutdownStart, Instant.now());
        fireEvent(new LifecycleEvent(LifecyclePhase.SHUTDOWN, previousPhase, null, shutdownDuration));
        log.info("Shutdown complete in {} ms", shutdownDuration.toMillis());
    }

    /**
     * Returns the current lifecycle phase.
     *
     * @return the current phase, or null if not yet started
     */
    public LifecyclePhase currentPhase() {
        return currentPhase.get();
    }

    /**
     * Returns true if the gateway is in the {@link LifecyclePhase#RUNNING} phase.
     */
    public boolean isRunning() {
        return currentPhase.get() == LifecyclePhase.RUNNING;
    }

    /**
     * Executes a phase transition with timing and event notification.
     */
    private void executePhaseTransition(LifecyclePhase from, LifecyclePhase to,
                                        PhaseAction action) throws LifecycleException {
        Instant start = Instant.now();
        log.info("Entering lifecycle phase: {}", to);

        if (from != null) {
            currentPhase.set(to);
        }

        try {
            action.execute();
        } catch (LifecycleException e) {
            throw e;
        } catch (Exception e) {
            throw new LifecycleException(to, "Phase " + to + " failed: " + e.getMessage(), e);
        }

        Duration duration = Duration.between(start, Instant.now());
        fireEvent(new LifecycleEvent(to, from, null, duration));
        log.info("Phase {} completed in {} ms", to, duration.toMillis());
    }

    /**
     * Starts all participants registered for the given phase, sorted by priority (ascending).
     */
    private void startParticipantsForPhase(LifecyclePhase phase) throws LifecycleException {
        List<LifecycleParticipant> phaseParticipants = participants.stream()
                .filter(p -> p.phase() == phase)
                .sorted(Comparator.comparingInt(LifecycleParticipant::priority))
                .toList();

        for (LifecycleParticipant participant : phaseParticipants) {
            Instant start = Instant.now();
            log.info("Starting participant: {} (priority={})", participant.name(), participant.priority());
            try {
                participant.start();
                synchronized (startedParticipants) {
                    startedParticipants.add(participant);
                }
                Duration duration = Duration.between(start, Instant.now());
                fireEvent(new LifecycleEvent(phase, null, participant.name(), duration));
                log.info("Started participant: {} in {} ms", participant.name(), duration.toMillis());
            } catch (Exception e) {
                throw new LifecycleException(phase, participant.name(),
                        "Failed to start participant: " + participant.name(), e);
            }
        }
    }

    /**
     * Notifies participants registered for the DRAIN phase.
     */
    private void notifyDrainParticipants() {
        List<LifecycleParticipant> drainParticipants = participants.stream()
                .filter(p -> p.phase() == LifecyclePhase.DRAIN)
                .sorted(Comparator.comparingInt(LifecycleParticipant::priority))
                .toList();

        for (LifecycleParticipant participant : drainParticipants) {
            try {
                log.info("Draining participant: {}", participant.name());
                participant.start(); // DRAIN participants use start() for drain activation
            } catch (Exception e) {
                log.warn("Error draining participant: {}", participant.name(), e);
            }
        }
    }

    /**
     * Stops all started participants in reverse priority order (highest priority stops first).
     */
    private void stopAllParticipants() {
        List<LifecycleParticipant> toStop;
        synchronized (startedParticipants) {
            toStop = new ArrayList<>(startedParticipants);
        }

        // Reverse: highest priority (started last) stops first
        toStop.sort(Comparator.comparingInt(LifecycleParticipant::priority).reversed());

        for (LifecycleParticipant participant : toStop) {
            try {
                Instant start = Instant.now();
                log.info("Stopping participant: {}", participant.name());
                participant.stop();
                Duration duration = Duration.between(start, Instant.now());
                log.info("Stopped participant: {} in {} ms", participant.name(), duration.toMillis());
            } catch (Exception e) {
                log.error("Error stopping participant: {}", participant.name(), e);
            }
        }

        synchronized (startedParticipants) {
            startedParticipants.clear();
        }
    }

    /**
     * Rolls back already-started components on startup failure.
     */
    private void rollbackStartedComponents() {
        log.warn("Rolling back {} started component(s) due to startup failure", startedParticipants.size());
        currentPhase.set(LifecyclePhase.SHUTDOWN);
        stopAllParticipants();
    }

    /**
     * Fires a lifecycle event to all registered listeners.
     */
    private void fireEvent(LifecycleEvent event) {
        for (LifecycleListener listener : listeners) {
            try {
                listener.onPhaseTransition(event);
            } catch (Exception e) {
                log.warn("Lifecycle listener threw exception for event {}: {}",
                        event.phase(), e.getMessage(), e);
            }
        }
    }

    /**
     * Internal functional interface for phase actions.
     */
    @FunctionalInterface
    private interface PhaseAction {
        void execute() throws Exception;
    }
}
