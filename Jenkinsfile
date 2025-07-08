pipeline {
    agent any
    environment {
        PMRS_OPT = '/nfs/gce/projects/pmrs/opt'
        RUSTUP_HOME = "${PMRS_OPT}/rustup"
        CARGO_HOME = "${PMRS_OPT}/cargo"
        PATH = "${CARGO_HOME}/bin:${env.PATH}"
    }
    stages {
        stage('prepare') {
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
                archiveArtifacts artifacts: 'target/release/build-pulse,report.html,data.db,config.toml'
            }
        }
    }
}

// vim: ts=4:sw=4:expandtab:
