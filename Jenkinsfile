pipeline {
    agent { label 'linux64' }
    parameters {
        booleanParam name: 'REUSE_DB', defaultValue: true, description: 'Whether or not to reuse the last `data.db`'
    }
    environment {
        PMRS_OPT = '/nfs/gce/projects/pmrs/opt'
        RUSTUP_HOME = "${PMRS_OPT}/rustup"
        CARGO_HOME = "${PMRS_OPT}/cargo"
        UV_HOME = "${PMRS_OPT}/uv-0.8.17"
        PATH = "${CARGO_HOME}/bin:${UV_HOME}:${env.PATH}"
    }
    stages {
        stage('prepare') {
            when {
                expression { return params.REUSE_DB }
            }
            steps {
                echo 'Copying cached database...'
                copyArtifacts projectName: currentBuild.fullProjectName, selector: lastCompleted(), filter: 'data.db', optional: true
            }
        }
        stage('build') {
            steps {
                echo 'Building build-pulse...'
                sh 'cargo build --release'
            }
        }
        stage('deploy') {
            steps {
                echo 'Running build-pulse...'
                sh './target/release/build-pulse -o report.html'
            }
        }
        stage('package') {
            steps {
                echo 'Archiving artifacts...'
                archiveArtifacts artifacts: 'target/release/build-pulse,report.html,data.db,config.toml,static/*,artifacts/*'
            }
        }
    }
}

// vim: ts=4:sw=4:expandtab:
