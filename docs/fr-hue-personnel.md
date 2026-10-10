# Version personnelle française et Hue

Modification locale de PlxNative 0.8.0, sous la licence GPL-3.0-or-later du projet.
Elle conserve l’identifiant `com.beb.plxnative` pour mettre à jour l’installation
existante. Une mise à jour officielle ultérieure remplacera ces modifications.

## Comportement prévu

- Français complet, choisi dans les paramètres ou automatiquement lorsque webOS
  utilise le français. Les préférences audio et de sous-titres Plex restent distinctes.
- Bouton « Lire avec Hue », ou « Reprendre avec Hue », à côté de la lecture normale.
  Il utilise le même média, la même source et la même position de reprise.
- Démarrage Hue après la première image vidéo du lecteur. Aucune connexion au
  Bridge ni aucun identifiant Hue n’est stocké dans PlxNative.
- Le service local existant choisit la zone Hue et conserve ses réglages.
- Sortie, arrière-plan ou erreur : fin de la session gérée. En cas de fermeture
  brutale, le service rend le contrôle après 20 secondes sans renouvellement.
- Une synchronisation déjà active avant la lecture reste active ensuite. Les
  réglages de luminosité modifiés pendant la lecture sont conservés.
- Si Hue est indisponible, la vidéo continue et une notification l’indique.

## Compilation depuis un PC Windows

Le contrôle des types ARM peut être fait depuis Windows avec Rust. La compilation
et l’assemblage du paquet utilisent le NDK webOS disponible pour Linux ARM64 ou
macOS. Le workflow manuel `.github/workflows/fr-hue-local.yml` fournit cette étape
sur `ubuntu-24.04-arm` ; il exécute les contrôles du projet, fabrique un IPK stable
sans fonctions de développement et joint les sources correspondantes. Il ne crée
aucune publication GitHub, aucun tag, et ne reçoit aucun secret Plex/Hue/SSH.
Les points d’envoi de télémétrie ne sont pas configurés pour cette version personnelle.

Après un envoi autorisé du code dans un dépôt GitHub, ouvrir Actions → Version
personnelle FR et Hue → Run workflow sur la branche modifiée. Télécharger
l’artefact `PlxNative-FR-Hue-personnel` après le succès des deux jobs.
Un échec des contrôles doit être corrigé avant installation.

## Service Hue complémentaire

`pkg/hue-companion/` contient le complément du service de test de couleurs déjà
installé avec lg-hue-sync. Les deux fichiers `tv_color_test.py` et
`tv_playback_hue.py` doivent être installés ensemble dans le dossier du service
existant, puis celui-ci redémarré. Le service reste sur la boucle locale
`127.0.0.1:8976`. PlxNative demande une session via `/api/playback/start`, la
renouvelle toutes les deux secondes et la termine via `/api/playback/stop`.
Chaque requête exige `X-LG-Hue-Test: 1` et son identifiant `X-LG-Hue-Session`.
Le service refuse les origines web extérieures et ne peut pas lancer de test de
couleurs simultanément. L’état antérieur est enregistré avant modification pour
permettre une restitution après redémarrage du service.

## Validation à terminer sur le téléviseur

Les contrôles de code ne prouvent pas la capture ou le rendu sur une vraie TV.
Sauvegarder l’application et ses données de connexion avant mise à jour, tenir
le verrou TV du dépôt pendant l’installation, comparer les empreintes du binaire
et vérifier : navigation télécommande, textes sans débordement, lecture normale,
reprise avec Hue, vidéo SDR, sortie du lecteur et arrêt brutal. HDR, Dolby Vision
et enchaînement d’épisodes nécessitent également un essai réel. Utiliser une vidéo
personnelle choisie par l’utilisateur, sans modifier son historique à son insu.

## Navigation des bibliothèques

Les bibliothèques Films et Séries restent visibles dans une seconde ligne fixe, rapprochée du menu principal. Toutes les bibliothèques du type choisi sont accessibles par gauche/droite dans cette ligne, y compris celles non épinglées ; il n'y a plus de capsule « +N ». La sélection défile horizontalement dans la zone visible ; la molette sur cette ligne avance ou recule entre les bibliothèques. Depuis une grille défilée, Retour rejoint les bibliothèques sans remonter le catalogue ; un deuxième Retour rejoint le menu principal. Le fond des deux lignes recouvre les affiches qui défilent dessous. Le pointeur ne peut pas sélectionner une affiche sous cette ligne fixe.
